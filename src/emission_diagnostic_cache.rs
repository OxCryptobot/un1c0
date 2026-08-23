use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::emission_diagnostic_attestation::{
    EmissionDiagnosticAttestation, EmissionDiagnosticAttestationError, VerifiedDiagnosticEvidence,
};
use crate::emission_diagnostic_stream::EmissionDiagnosticStream;
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::SemanticCacheKey;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;

pub const MAX_DIAGNOSTIC_EVIDENCE_CACHE_ENTRIES: usize = 1_024;
pub const DEFAULT_DIAGNOSTIC_EVIDENCE_CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_KEY_DOMAIN: &[u8] = b"un1c0/phase76/diagnostic-evidence-cache-key/v1";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiagnosticEvidenceCacheConfigError {
    #[error("diagnostic evidence cache capacity must be greater than zero")]
    ZeroCapacity,
    #[error("diagnostic evidence cache capacity {capacity} exceeds maximum {maximum}")]
    CapacityTooLarge { capacity: usize, maximum: usize },
    #[error("diagnostic evidence cache byte budget must be greater than zero")]
    ZeroByteBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticEvidenceCacheKey([u8; 32]);

impl DiagnosticEvidenceCacheKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiagnosticEvidenceCacheMetrics {
    pub capacity: usize,
    pub max_bytes: usize,
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub invalidations: u64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    key: DiagnosticEvidenceCacheKey,
    evidence: Arc<VerifiedDiagnosticEvidence>,
    trust_epoch: u64,
    bytes: usize,
}

#[derive(Debug, Default)]
struct CacheState {
    entries: VecDeque<CacheEntry>,
    bytes: usize,
    hits: u64,
    misses: u64,
    insertions: u64,
    evictions: u64,
    invalidations: u64,
}

#[derive(Debug, Clone)]
pub struct DiagnosticEvidenceCache {
    capacity: usize,
    max_bytes: usize,
    state: Arc<Mutex<CacheState>>,
}

impl DiagnosticEvidenceCache {
    pub fn new(
        capacity: usize,
        max_bytes: usize,
    ) -> Result<Self, DiagnosticEvidenceCacheConfigError> {
        if capacity == 0 {
            return Err(DiagnosticEvidenceCacheConfigError::ZeroCapacity);
        }
        if capacity > MAX_DIAGNOSTIC_EVIDENCE_CACHE_ENTRIES {
            return Err(DiagnosticEvidenceCacheConfigError::CapacityTooLarge {
                capacity,
                maximum: MAX_DIAGNOSTIC_EVIDENCE_CACHE_ENTRIES,
            });
        }
        if max_bytes == 0 {
            return Err(DiagnosticEvidenceCacheConfigError::ZeroByteBudget);
        }
        Ok(Self {
            capacity,
            max_bytes,
            state: Arc::new(Mutex::new(CacheState::default())),
        })
    }

    pub fn with_default_budget(
        capacity: usize,
    ) -> Result<Self, DiagnosticEvidenceCacheConfigError> {
        Self::new(capacity, DEFAULT_DIAGNOSTIC_EVIDENCE_CACHE_BYTES)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn key_for(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        trust_epoch: u64,
    ) -> Result<DiagnosticEvidenceCacheKey, EmissionDiagnosticAttestationError> {
        let attestation_bytes = attestation.to_json()?;
        let mut hasher = Sha256::new();
        hasher.update(CACHE_KEY_DOMAIN);
        feed_bytes(&mut hasher, &attestation_bytes);
        feed_bytes(&mut hasher, &stream.stream_digest());
        feed_u64(&mut hasher, stream.stream_id());
        feed_u64(&mut hasher, stream.batch_id());
        feed_str(&mut hasher, profile.target.label());
        feed_key(&mut hasher, stream.profile_key());
        feed_u64(&mut hasher, envelope.batch_id());
        feed_key(&mut hasher, envelope.profile_key());
        feed_unit_roots(&mut hasher, stream.unit_roots());
        feed_unit_roots(
            &mut hasher,
            &envelope
                .units()
                .iter()
                .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
                .collect(),
        );
        feed_u64(&mut hasher, trust_epoch);
        Ok(DiagnosticEvidenceCacheKey(hasher.finalize().into()))
    }

    pub fn lookup(
        &self,
        key: DiagnosticEvidenceCacheKey,
    ) -> Option<Arc<VerifiedDiagnosticEvidence>> {
        let mut state = self.lock_state();
        let position = state.entries.iter().position(|entry| entry.key == key);
        match position {
            Some(position) => {
                let entry = state
                    .entries
                    .remove(position)
                    .expect("cache position exists");
                state.entries.push_back(entry.clone());
                state.hits = state.hits.saturating_add(1);
                Some(entry.evidence)
            }
            None => {
                state.misses = state.misses.saturating_add(1);
                None
            }
        }
    }

    pub fn insert(
        &self,
        key: DiagnosticEvidenceCacheKey,
        evidence: Arc<VerifiedDiagnosticEvidence>,
    ) -> bool {
        let bytes = evidence.canonical().canonical_stream_bytes().len();
        if bytes > self.max_bytes {
            return false;
        }
        let mut state = self.lock_state();
        if let Some(position) = state.entries.iter().position(|entry| entry.key == key) {
            let previous = state
                .entries
                .remove(position)
                .expect("cache position exists");
            state.bytes = state.bytes.saturating_sub(previous.bytes);
        }
        while state.entries.len() >= self.capacity
            || state.bytes.saturating_add(bytes) > self.max_bytes
        {
            let Some(evicted) = state.entries.pop_front() else {
                break;
            };
            state.bytes = state.bytes.saturating_sub(evicted.bytes);
            state.evictions = state.evictions.saturating_add(1);
        }
        state.bytes = state.bytes.saturating_add(bytes);
        state.entries.push_back(CacheEntry {
            key,
            trust_epoch: evidence.trust_epoch(),
            evidence,
            bytes,
        });
        state.insertions = state.insertions.saturating_add(1);
        true
    }

    pub fn invalidate_key(&self, key: DiagnosticEvidenceCacheKey) -> bool {
        let mut state = self.lock_state();
        let Some(position) = state.entries.iter().position(|entry| entry.key == key) else {
            return false;
        };
        let entry = state
            .entries
            .remove(position)
            .expect("cache position exists");
        state.bytes = state.bytes.saturating_sub(entry.bytes);
        state.invalidations = state.invalidations.saturating_add(1);
        true
    }

    pub fn invalidate_trust_epoch(&self, trust_epoch: u64) -> usize {
        let mut state = self.lock_state();
        let mut removed = 0usize;
        let mut retained = VecDeque::with_capacity(state.entries.len());
        while let Some(entry) = state.entries.pop_front() {
            if entry.trust_epoch == trust_epoch {
                retained.push_back(entry);
            } else {
                state.bytes = state.bytes.saturating_sub(entry.bytes);
                state.invalidations = state.invalidations.saturating_add(1);
                removed += 1;
            }
        }
        state.entries = retained;
        removed
    }

    pub fn clear(&self) -> usize {
        let mut state = self.lock_state();
        let removed = state.entries.len();
        if removed > 0 {
            state.invalidations = state.invalidations.saturating_add(removed as u64);
        }
        state.entries.clear();
        state.bytes = 0;
        removed
    }

    pub fn metrics(&self) -> DiagnosticEvidenceCacheMetrics {
        let state = self.lock_state();
        DiagnosticEvidenceCacheMetrics {
            capacity: self.capacity,
            max_bytes: self.max_bytes,
            entries: state.entries.len(),
            bytes: state.bytes,
            hits: state.hits,
            misses: state.misses,
            insertions: state.insertions,
            evictions: state.evictions,
            invalidations: state.invalidations,
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn feed_unit_roots(hasher: &mut Sha256, roots: &BTreeMap<SemanticUnitId, SemanticCacheKey>) {
    feed_u64(hasher, roots.len() as u64);
    for (unit, root) in roots {
        feed_str(hasher, unit.as_str());
        feed_key(hasher, *root);
    }
}

fn feed_key(hasher: &mut Sha256, key: SemanticCacheKey) {
    hasher.update(key.as_bytes());
}

fn feed_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_le_bytes());
}

fn feed_str(hasher: &mut Sha256, value: &str) {
    feed_bytes(hasher, value.as_bytes());
}

fn feed_bytes(hasher: &mut Sha256, value: &[u8]) {
    feed_u64(hasher, value.len() as u64);
    hasher.update(value);
}
