use crate::cross_process_ownership::{
    CrossProcessOwnershipStore, OwnershipClaim, OwnershipError, OwnershipRecord,
    OwnershipWritePermit,
};
use crate::replicated_durability::{
    CasCommitOutcome, CasDurabilitySnapshotStore, CasPreAdmissionContext, CasState,
    CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicatedDurabilityError,
    SingleWriterCasStore,
};
use ed25519_dalek::VerifyingKey;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OwnershipBoundCasError {
    #[error("ownership operation failed: {0}")]
    Ownership(#[from] OwnershipError),
    #[error("replicated CAS operation failed: {0}")]
    Cas(#[from] ReplicatedDurabilityError),
    #[error("ownership permit is stale: {0}")]
    StalePermit(String),
    #[error("ownership and CAS state diverged: {0}")]
    StateDiverged(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipBoundCasReceipt {
    pub outcome: CasCommitOutcome,
    pub ownership_record_hash: String,
    pub ownership_epoch: u64,
    pub generation: u64,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct OwnershipBoundCasCoordinator {
    ownership: CrossProcessOwnershipStore,
    cas: SingleWriterCasStore,
    snapshot_store: CasDurabilitySnapshotStore,
}

impl OwnershipBoundCasCoordinator {
    pub fn new(
        ownership_path: impl Into<PathBuf>,
        cas_snapshot_path: impl Into<PathBuf>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        required_quorum: usize,
        max_committed_requests: usize,
    ) -> Result<Self, OwnershipBoundCasError> {
        Ok(Self {
            ownership: CrossProcessOwnershipStore::new(
                ownership_path,
                cluster_id,
                resource_id,
                snapshot_id,
            )?,
            cas: SingleWriterCasStore::new(
                cluster_id,
                resource_id,
                snapshot_id,
                required_quorum,
                max_committed_requests,
            )?,
            snapshot_store: CasDurabilitySnapshotStore::new(
                cas_snapshot_path,
                cluster_id,
                resource_id,
                snapshot_id,
            )?,
        })
    }

    pub fn register_owner(
        &mut self,
        owner_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), OwnershipBoundCasError> {
        self.ownership.register_owner(owner_id, verifying_key)?;
        self.cas.register_writer(owner_id, verifying_key)?;
        Ok(())
    }

    pub fn register_replica(
        &mut self,
        replica_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), OwnershipBoundCasError> {
        self.cas.register_replica(replica_id, verifying_key)?;
        Ok(())
    }

    pub fn acquire(
        &self,
        claim: OwnershipClaim,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipBoundCasError> {
        Ok(self.ownership.acquire(claim, current_tick)?)
    }

    pub fn current_owner(&self) -> Result<Option<OwnershipRecord>, OwnershipBoundCasError> {
        Ok(self.ownership.current()?)
    }

    pub fn cas_state(&self) -> &CasState {
        self.cas.state()
    }

    pub fn pre_admission_context(&self) -> Result<CasPreAdmissionContext, OwnershipBoundCasError> {
        Ok(self.cas.pre_admission_context()?)
    }

    pub fn admit_write(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        current_tick: u64,
    ) -> Result<OwnershipWritePermit, OwnershipBoundCasError> {
        Ok(self.ownership.admit_write(
            owner_id,
            process_instance,
            ownership_epoch,
            expected_record_hash,
            current_tick,
        )?)
    }

    pub fn renew(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        new_expiry_tick: u64,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipBoundCasError> {
        Ok(self.ownership.renew(
            owner_id,
            process_instance,
            ownership_epoch,
            expected_record_hash,
            new_expiry_tick,
            current_tick,
        )?)
    }

    pub fn release(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipBoundCasError> {
        Ok(self.ownership.release(
            owner_id,
            process_instance,
            ownership_epoch,
            expected_record_hash,
            current_tick,
        )?)
    }

    pub fn commit_owned(
        &mut self,
        permit: OwnershipWritePermit,
        request: CasWriteRequest,
        acknowledgements: &[ReplicaDurabilityAcknowledgement],
        current_tick: u64,
    ) -> Result<OwnershipBoundCasReceipt, OwnershipBoundCasError> {
        if permit.owner_id != request.writer_id || permit.ownership_epoch != request.writer_epoch {
            return Err(OwnershipBoundCasError::StalePermit(
                "CAS request writer identity or ownership epoch differs from the permit".into(),
            ));
        }

        let ownership = &self.ownership;
        let cas = &mut self.cas;
        let snapshot_store = &self.snapshot_store;
        let proposed_generation = request.proposed_generation;
        let proposed_hash = request.proposed_hash.clone();
        ownership.with_owned_lock(&permit, current_tick, |record| {
            Self::refresh_cas_state(cas, snapshot_store)?;
            if cas.state().generation != record.generation
                || cas.state().content_hash != record.content_hash
            {
                return Err(OwnershipBoundCasError::StateDiverged(
                    "ownership record does not match the durable CAS state".into(),
                ));
            }

            let before = cas.snapshot()?;
            let outcome = cas.commit(request, acknowledgements, current_tick)?;
            match &outcome {
                CasCommitOutcome::Idempotent(receipt) => {
                    if receipt.generation != record.generation
                        || receipt.content_hash != record.content_hash
                    {
                        return Err(OwnershipBoundCasError::StateDiverged(
                            "idempotent receipt does not match the active ownership record".into(),
                        ));
                    }
                    let generation = receipt.generation;
                    let content_hash = receipt.content_hash.clone();
                    Ok(OwnershipBoundCasReceipt {
                        outcome,
                        ownership_record_hash: record.record_hash.clone(),
                        ownership_epoch: record.ownership_epoch,
                        generation,
                        content_hash,
                    })
                }
                CasCommitOutcome::Committed(receipt) => {
                    if receipt.generation != proposed_generation
                        || receipt.content_hash != proposed_hash
                    {
                        cas.restore(before)?;
                        return Err(OwnershipBoundCasError::StateDiverged(
                            "CAS receipt does not match the ownership transition".into(),
                        ));
                    }
                    let after = cas.snapshot()?;
                    if let Err(error) = snapshot_store.save(&after) {
                        cas.restore(before.clone())?;
                        if let Err(rollback_error) = snapshot_store.save(&before) {
                            return Err(OwnershipBoundCasError::StateDiverged(format!(
                                "CAS snapshot rollback failed after persistence error: {error}; rollback: {rollback_error}"
                            )));
                        }
                        return Err(error.into());
                    }
                    let mut updated = record.clone();
                    updated.generation = receipt.generation;
                    updated.content_hash = receipt.content_hash.clone();
                    updated.recompute_hash()?;
                    if let Err(error) = ownership.persist_owned_record(&updated) {
                        cas.restore(before.clone())?;
                        if let Err(rollback_error) = snapshot_store.save(&before) {
                            return Err(OwnershipBoundCasError::StateDiverged(format!(
                                "ownership rollback failed after record persistence error: {error}; rollback: {rollback_error}"
                            )));
                        }
                        return Err(error.into());
                    }
                    let generation = receipt.generation;
                    let content_hash = receipt.content_hash.clone();
                    Ok(OwnershipBoundCasReceipt {
                        outcome,
                        ownership_record_hash: updated.record_hash,
                        ownership_epoch: updated.ownership_epoch,
                        generation,
                        content_hash,
                    })
                }
            }
        })
    }

    fn refresh_cas_state(
        cas: &mut SingleWriterCasStore,
        snapshot_store: &CasDurabilitySnapshotStore,
    ) -> Result<(), OwnershipBoundCasError> {
        if let Some(snapshot) = snapshot_store.load()? {
            cas.restore(snapshot)?;
        }
        Ok(())
    }
}
