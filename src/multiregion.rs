use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_REGIONS: usize = 8;
const MAX_NODES: usize = 64;
const MAX_EVENTS: usize = 4096;
const MAX_TICKS: u64 = 1_000_000;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 4096;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MultiRegionSimulationError {
    #[error("invalid simulation configuration: {0}")]
    InvalidConfiguration(String),
    #[error("simulation invariant violated: {0}")]
    InvariantViolation(String),
    #[error("simulation snapshot is invalid: {0}")]
    InvalidSnapshot(String),
    #[error("simulation limit exceeded: {0}")]
    LimitExceeded(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionId(String);

impl RegionId {
    pub fn new(value: &str) -> Result<Self, MultiRegionSimulationError> {
        validate_identifier(value, "region")?;
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(value: &str) -> Result<Self, MultiRegionSimulationError> {
        validate_identifier(value, "node")?;
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionNode {
    pub node_id: NodeId,
    pub region_id: RegionId,
}

impl RegionNode {
    pub fn new(node_id: &str, region_id: &str) -> Result<Self, MultiRegionSimulationError> {
        Ok(Self {
            node_id: NodeId::new(node_id)?,
            region_id: RegionId::new(region_id)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LinkFault {
    Healthy,
    Drop,
    Delay { ticks: u64 },
    Duplicate,
    Reorder,
    Corrupt,
}

impl LinkFault {
    fn validate(&self) -> Result<(), MultiRegionSimulationError> {
        if let Self::Delay { ticks } = self {
            if *ticks == 0 || *ticks > MAX_TICKS {
                return Err(MultiRegionSimulationError::InvalidConfiguration(
                    "link delay must be between one tick and the simulation bound".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiRegionSimulationConfig {
    pub scenario_id: String,
    pub seed: u64,
    pub regions: Vec<RegionId>,
    pub nodes: Vec<RegionNode>,
    pub quorum_size: usize,
    pub max_ticks: u64,
    pub max_clock_skew_ticks: u64,
}

impl MultiRegionSimulationConfig {
    pub fn three_region(scenario_id: &str, seed: u64) -> Result<Self, MultiRegionSimulationError> {
        let regions = vec![
            RegionId::new("region-a")?,
            RegionId::new("region-b")?,
            RegionId::new("region-c")?,
        ];
        let nodes = vec![
            RegionNode::new("node-a1", "region-a")?,
            RegionNode::new("node-a2", "region-a")?,
            RegionNode::new("node-b1", "region-b")?,
            RegionNode::new("node-b2", "region-b")?,
            RegionNode::new("node-c1", "region-c")?,
        ];
        let config = Self {
            scenario_id: scenario_id.to_string(),
            seed,
            regions,
            nodes,
            quorum_size: 3,
            max_ticks: 256,
            max_clock_skew_ticks: 2,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), MultiRegionSimulationError> {
        validate_identifier(&self.scenario_id, "scenario")?;
        if self.regions.is_empty() || self.regions.len() > MAX_REGIONS {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "region count is outside the bounded range".into(),
            ));
        }
        if self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "node count is outside the bounded range".into(),
            ));
        }
        if self.quorum_size == 0 || self.quorum_size > self.nodes.len() {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "quorum size is outside the node count".into(),
            ));
        }
        if self.max_ticks == 0 || self.max_ticks > MAX_TICKS {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "maximum ticks are outside the bounded range".into(),
            ));
        }
        if self.max_clock_skew_ticks == 0 || self.max_clock_skew_ticks > self.max_ticks {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "maximum clock skew is outside the bounded range".into(),
            ));
        }
        let regions: BTreeSet<_> = self.regions.iter().cloned().collect();
        if regions.len() != self.regions.len() {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "regions must be unique".into(),
            ));
        }
        let mut nodes = BTreeSet::new();
        for node in &self.nodes {
            if !regions.contains(&node.region_id) {
                return Err(MultiRegionSimulationError::InvalidConfiguration(
                    "node references an unknown region".into(),
                ));
            }
            if !nodes.insert(node.node_id.clone()) {
                return Err(MultiRegionSimulationError::InvalidConfiguration(
                    "nodes must be unique".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SimulationEventKind {
    DeliveryAttempted,
    AcknowledgementScheduled,
    AcknowledgementDropped,
    AcknowledgementCorrupted,
    AcknowledgementDelivered,
    FaultInjected,
    FenceRecorded,
    ObserverReportAccepted,
    TransferAccepted,
    SnapshotCreated,
    SnapshotRestored,
    QueueCommitted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimulationEvent {
    pub sequence: u64,
    pub tick: u64,
    pub kind: SimulationEventKind,
    pub from: Option<NodeId>,
    pub to: Option<NodeId>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnershipFenceObservation {
    pub owner_id: NodeId,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub observed_tick: u64,
    pub reachable_members: usize,
    pub required_members: usize,
    pub reason: String,
}

impl OwnershipFenceObservation {
    fn validate(&self) -> Result<(), MultiRegionSimulationError> {
        if self.owner_term == 0 || self.ownership_epoch == 0 {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "fence term and epoch must be positive".into(),
            ));
        }
        if self.reachable_members >= self.required_members || self.required_members == 0 {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "fence must represent a quorum loss".into(),
            ));
        }
        if self.reason.is_empty()
            || self.reason.len() > MAX_REASON_BYTES
            || self.reason.chars().any(char::is_control)
        {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "fence reason is empty, oversized, or contains control characters".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailoverTransfer {
    pub previous_owner_id: NodeId,
    pub new_owner_id: NodeId,
    pub owner_term: u64,
    pub ownership_epoch: u64,
}

impl FailoverTransfer {
    fn validate(&self) -> Result<(), MultiRegionSimulationError> {
        if self.owner_term == 0 || self.ownership_epoch == 0 {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "transfer term and epoch must be positive".into(),
            ));
        }
        if self.previous_owner_id == self.new_owner_id {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "transfer must change owner".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingAcknowledgement {
    deliver_tick: u64,
    sequence: u64,
    sender: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiRegionSnapshot {
    pub config: MultiRegionSimulationConfig,
    pub tick: u64,
    pub active_owner_id: NodeId,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub clock_skew_ticks: u64,
    pub fence: Option<OwnershipFenceObservation>,
    pub queue_sequence: u64,
    pub committed: bool,
    pub acknowledgements: BTreeSet<NodeId>,
    pub observer_reports: BTreeSet<NodeId>,
    pub links: BTreeMap<(NodeId, NodeId), LinkFault>,
    pending: Vec<PendingAcknowledgement>,
    events: Vec<SimulationEvent>,
    next_event_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiRegionSimulationReport {
    pub scenario_id: String,
    pub seed: u64,
    pub trace_digest: String,
    pub safety_passed: bool,
    pub liveness_passed: bool,
    pub committed: bool,
    pub fenced: bool,
    pub active_owner_id: NodeId,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub events: usize,
    pub dropped_acknowledgements: usize,
    pub delivered_acknowledgements: usize,
    pub invariant_failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MultiRegionFailoverSimulator {
    snapshot: MultiRegionSnapshot,
    invariant_failures: Vec<String>,
    dropped_acknowledgements: usize,
    delivered_acknowledgements: usize,
}

impl MultiRegionFailoverSimulator {
    pub fn new(config: MultiRegionSimulationConfig) -> Result<Self, MultiRegionSimulationError> {
        config.validate()?;
        let initial_owner = config
            .nodes
            .first()
            .map(|node| node.node_id.clone())
            .ok_or_else(|| MultiRegionSimulationError::InvalidConfiguration("no nodes".into()))?;
        let mut links = BTreeMap::new();
        for from in &config.nodes {
            for to in &config.nodes {
                if from.node_id != to.node_id {
                    links.insert(
                        (from.node_id.clone(), to.node_id.clone()),
                        LinkFault::Healthy,
                    );
                }
            }
        }
        Ok(Self {
            snapshot: MultiRegionSnapshot {
                config,
                tick: 0,
                active_owner_id: initial_owner,
                owner_term: 1,
                ownership_epoch: 1,
                clock_skew_ticks: 0,
                fence: None,
                queue_sequence: 1,
                committed: false,
                acknowledgements: BTreeSet::new(),
                observer_reports: BTreeSet::new(),
                links,
                pending: Vec::new(),
                events: Vec::new(),
                next_event_sequence: 0,
            },
            invariant_failures: Vec::new(),
            dropped_acknowledgements: 0,
            delivered_acknowledgements: 0,
        })
    }

    pub fn from_snapshot(
        snapshot: MultiRegionSnapshot,
    ) -> Result<Self, MultiRegionSimulationError> {
        snapshot.config.validate()?;
        if snapshot.tick > snapshot.config.max_ticks
            || snapshot.owner_term == 0
            || snapshot.ownership_epoch == 0
            || snapshot.clock_skew_ticks > snapshot.config.max_clock_skew_ticks
            || snapshot.queue_sequence == 0
        {
            return Err(MultiRegionSimulationError::InvalidSnapshot(
                "snapshot counters are outside bounds".into(),
            ));
        }
        if let Some(fence) = &snapshot.fence {
            fence.validate()?;
        }
        Ok(Self {
            snapshot,
            invariant_failures: Vec::new(),
            dropped_acknowledgements: 0,
            delivered_acknowledgements: 0,
        })
    }

    pub fn snapshot(&mut self) -> Result<MultiRegionSnapshot, MultiRegionSimulationError> {
        self.record(
            SimulationEventKind::SnapshotCreated,
            None,
            None,
            "durable snapshot created",
        );
        self.assert_invariants();
        Ok(self.snapshot.clone())
    }

    pub fn restore_snapshot(
        &mut self,
        snapshot: MultiRegionSnapshot,
    ) -> Result<(), MultiRegionSimulationError> {
        let restored = Self::from_snapshot(snapshot)?;
        self.snapshot = restored.snapshot;
        self.invariant_failures.extend(restored.invariant_failures);
        self.record(
            SimulationEventKind::SnapshotRestored,
            None,
            None,
            "durable snapshot restored",
        );
        self.assert_invariants();
        Ok(())
    }

    pub fn inject_link_fault(
        &mut self,
        from: &str,
        to: &str,
        fault: LinkFault,
    ) -> Result<(), MultiRegionSimulationError> {
        self.set_link_fault(from, to, fault.clone())?;
        self.record(
            SimulationEventKind::FaultInjected,
            Some(NodeId::new(from)?),
            Some(NodeId::new(to)?),
            "replay fault injected",
        );
        Ok(())
    }

    pub fn set_link_fault(
        &mut self,
        from: &str,
        to: &str,
        fault: LinkFault,
    ) -> Result<(), MultiRegionSimulationError> {
        let from = NodeId::new(from)?;
        let to = NodeId::new(to)?;
        if from == to
            || !self
                .snapshot
                .links
                .contains_key(&(from.clone(), to.clone()))
        {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "link endpoints must be distinct configured nodes".into(),
            ));
        }
        fault.validate()?;
        self.snapshot.links.insert((from, to), fault);
        Ok(())
    }

    pub fn partition_regions(
        &mut self,
        left_region: &str,
        right_region: &str,
    ) -> Result<(), MultiRegionSimulationError> {
        let left = RegionId::new(left_region)?;
        let right = RegionId::new(right_region)?;
        for from in &self.snapshot.config.nodes {
            for to in &self.snapshot.config.nodes {
                if from.region_id == left && to.region_id == right {
                    self.snapshot
                        .links
                        .insert((from.node_id.clone(), to.node_id.clone()), LinkFault::Drop);
                }
                if from.region_id == right && to.region_id == left {
                    self.snapshot
                        .links
                        .insert((from.node_id.clone(), to.node_id.clone()), LinkFault::Drop);
                }
            }
        }
        Ok(())
    }

    pub fn heal_all_links(&mut self) {
        for fault in self.snapshot.links.values_mut() {
            *fault = LinkFault::Healthy;
        }
    }

    pub fn set_clock_skew_ticks(
        &mut self,
        skew_ticks: u64,
    ) -> Result<(), MultiRegionSimulationError> {
        if skew_ticks > MAX_TICKS {
            return Err(MultiRegionSimulationError::LimitExceeded(
                "clock skew exceeds simulation bound".into(),
            ));
        }
        self.snapshot.clock_skew_ticks = skew_ticks;
        self.assert_invariants();
        Ok(())
    }

    pub fn submit_observer_quorum_loss(
        &mut self,
        observer_id: &str,
        reachable_members: usize,
        reason: &str,
    ) -> Result<bool, MultiRegionSimulationError> {
        let observer = NodeId::new(observer_id)?;
        if observer == self.snapshot.active_owner_id
            || !self
                .snapshot
                .config
                .nodes
                .iter()
                .any(|node| node.node_id == observer)
        {
            return Err(MultiRegionSimulationError::InvalidConfiguration(
                "observer must be a configured non-owner node".into(),
            ));
        }
        match self
            .snapshot
            .links
            .get(&(observer.clone(), self.snapshot.active_owner_id.clone()))
        {
            Some(LinkFault::Drop) | Some(LinkFault::Corrupt) | None => return Ok(false),
            Some(_) => {}
        }
        self.snapshot.observer_reports.insert(observer.clone());
        self.record(
            SimulationEventKind::ObserverReportAccepted,
            Some(observer),
            Some(self.snapshot.active_owner_id.clone()),
            "observer quorum-loss report accepted",
        );
        if self.snapshot.observer_reports.len() + 1 >= self.snapshot.config.quorum_size {
            self.record_quorum_loss(reachable_members, reason)?;
            return Ok(true);
        }
        self.assert_invariants();
        Ok(false)
    }

    pub fn record_quorum_loss(
        &mut self,
        reachable_members: usize,
        reason: &str,
    ) -> Result<(), MultiRegionSimulationError> {
        let observation = OwnershipFenceObservation {
            owner_id: self.snapshot.active_owner_id.clone(),
            owner_term: self.snapshot.owner_term,
            ownership_epoch: self.snapshot.ownership_epoch,
            observed_tick: self.snapshot.tick,
            reachable_members,
            required_members: self.snapshot.config.quorum_size,
            reason: reason.to_string(),
        };
        observation.validate()?;
        if let Some(existing) = &self.snapshot.fence {
            if existing.observed_tick > observation.observed_tick {
                return Ok(());
            }
            if existing.observed_tick == observation.observed_tick && existing != &observation {
                self.invariant_failures
                    .push("conflicting same-tick fence observation".into());
                return Err(MultiRegionSimulationError::InvariantViolation(
                    "conflicting same-tick fence observation".into(),
                ));
            }
        }
        self.snapshot.fence = Some(observation);
        self.record(
            SimulationEventKind::FenceRecorded,
            Some(self.snapshot.active_owner_id.clone()),
            None,
            "quorum-loss fence recorded",
        );
        self.assert_invariants();
        Ok(())
    }

    pub fn attempt_delivery(&mut self) -> Result<bool, MultiRegionSimulationError> {
        self.record(
            SimulationEventKind::DeliveryAttempted,
            Some(self.snapshot.active_owner_id.clone()),
            None,
            "queue head delivery attempted",
        );
        if self.snapshot.committed {
            self.assert_invariants();
            return Ok(true);
        }
        if self.snapshot.fence.is_some() {
            self.assert_invariants();
            return Ok(false);
        }
        let owner = self.snapshot.active_owner_id.clone();
        let mut reachable = 1usize;
        let peers: Vec<_> = self
            .snapshot
            .config
            .nodes
            .iter()
            .filter(|node| node.node_id != owner)
            .map(|node| node.node_id.clone())
            .collect();
        for peer in peers {
            match self.snapshot.links.get(&(owner.clone(), peer.clone())) {
                Some(LinkFault::Healthy)
                | Some(LinkFault::Duplicate)
                | Some(LinkFault::Reorder) => {
                    reachable += 1;
                    self.schedule_ack(peer.clone(), 0);
                    if matches!(
                        self.snapshot.links.get(&(owner.clone(), peer.clone())),
                        Some(LinkFault::Duplicate)
                    ) {
                        self.schedule_ack(peer, 0);
                    }
                }
                Some(LinkFault::Delay { ticks }) => {
                    reachable += 1;
                    self.schedule_ack(peer, *ticks);
                }
                Some(LinkFault::Drop) => {
                    self.dropped_acknowledgements += 1;
                    self.record(
                        SimulationEventKind::AcknowledgementDropped,
                        Some(owner.clone()),
                        Some(peer),
                        "directed link dropped acknowledgement",
                    );
                }
                Some(LinkFault::Corrupt) => {
                    self.dropped_acknowledgements += 1;
                    self.record(
                        SimulationEventKind::AcknowledgementCorrupted,
                        Some(owner.clone()),
                        Some(peer),
                        "directed link corrupted acknowledgement",
                    );
                }
                None => {
                    return Err(MultiRegionSimulationError::InvalidSnapshot(
                        "missing directed link".into(),
                    ));
                }
            }
        }
        if reachable < self.snapshot.config.quorum_size {
            self.record_quorum_loss(reachable, "delivery quorum unavailable")?;
            return Ok(false);
        }
        self.process_pending_events()?;
        self.assert_invariants();
        Ok(self.snapshot.committed)
    }

    pub fn advance_ticks(&mut self, ticks: u64) -> Result<(), MultiRegionSimulationError> {
        let next = self
            .snapshot
            .tick
            .checked_add(ticks)
            .ok_or_else(|| MultiRegionSimulationError::LimitExceeded("tick overflow".into()))?;
        if next > self.snapshot.config.max_ticks {
            return Err(MultiRegionSimulationError::LimitExceeded(
                "simulation tick bound exceeded".into(),
            ));
        }
        self.snapshot.tick = next;
        self.process_pending_events()?;
        self.assert_invariants();
        Ok(())
    }

    pub fn accept_transfer(
        &mut self,
        transfer: FailoverTransfer,
    ) -> Result<(), MultiRegionSimulationError> {
        transfer.validate()?;
        if self.snapshot.clock_skew_ticks > self.snapshot.config.max_clock_skew_ticks {
            return Err(MultiRegionSimulationError::InvariantViolation(
                "clock uncertainty blocks ownership transfer".into(),
            ));
        }
        if transfer.previous_owner_id != self.snapshot.active_owner_id
            || transfer.owner_term <= self.snapshot.owner_term
            || transfer.ownership_epoch <= self.snapshot.ownership_epoch
            || !self
                .snapshot
                .config
                .nodes
                .iter()
                .any(|node| node.node_id == transfer.new_owner_id)
        {
            return Err(MultiRegionSimulationError::InvariantViolation(
                "transfer is stale, misbound, or targets an unknown owner".into(),
            ));
        }
        self.snapshot.active_owner_id = transfer.new_owner_id.clone();
        self.snapshot.owner_term = transfer.owner_term;
        self.snapshot.ownership_epoch = transfer.ownership_epoch;
        self.snapshot.fence = None;
        self.snapshot.acknowledgements.clear();
        self.snapshot.observer_reports.clear();
        self.snapshot.pending.clear();
        self.record(
            SimulationEventKind::TransferAccepted,
            Some(transfer.previous_owner_id),
            Some(transfer.new_owner_id),
            "higher-term ownership transfer accepted",
        );
        self.assert_invariants();
        Ok(())
    }

    pub fn report(&self) -> MultiRegionSimulationReport {
        let safety_passed = self.invariant_failures.is_empty() && self.safety_state_is_valid();
        let liveness_passed = self.snapshot.committed;
        MultiRegionSimulationReport {
            scenario_id: self.snapshot.config.scenario_id.clone(),
            seed: self.snapshot.config.seed,
            trace_digest: self.trace_digest(),
            safety_passed,
            liveness_passed,
            committed: self.snapshot.committed,
            fenced: self.snapshot.fence.is_some(),
            active_owner_id: self.snapshot.active_owner_id.clone(),
            owner_term: self.snapshot.owner_term,
            ownership_epoch: self.snapshot.ownership_epoch,
            events: self.snapshot.events.len(),
            dropped_acknowledgements: self.dropped_acknowledgements,
            delivered_acknowledgements: self.delivered_acknowledgements,
            invariant_failures: self.invariant_failures.clone(),
        }
    }

    pub fn trace_digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.snapshot.events).unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }

    pub fn events(&self) -> &[SimulationEvent] {
        &self.snapshot.events
    }

    pub fn current_tick(&self) -> u64 {
        self.snapshot.tick
    }

    fn schedule_ack(&mut self, sender: NodeId, delay: u64) {
        let deliver_tick = self.snapshot.tick.saturating_add(delay);
        self.snapshot.pending.push(PendingAcknowledgement {
            deliver_tick,
            sequence: self.snapshot.queue_sequence,
            sender: sender.clone(),
        });
        self.record(
            SimulationEventKind::AcknowledgementScheduled,
            Some(self.snapshot.active_owner_id.clone()),
            Some(sender),
            "acknowledgement scheduled",
        );
    }

    fn process_pending_events(&mut self) -> Result<(), MultiRegionSimulationError> {
        self.snapshot.pending.sort_by(|left, right| {
            (left.deliver_tick, &left.sender).cmp(&(right.deliver_tick, &right.sender))
        });
        let mut remaining = Vec::new();
        let pending = std::mem::take(&mut self.snapshot.pending);
        for event in pending {
            if event.deliver_tick > self.snapshot.tick {
                remaining.push(event);
                continue;
            }
            if self.snapshot.acknowledgements.insert(event.sender.clone()) {
                self.delivered_acknowledgements += 1;
                self.record(
                    SimulationEventKind::AcknowledgementDelivered,
                    Some(event.sender.clone()),
                    Some(self.snapshot.active_owner_id.clone()),
                    "acknowledgement delivered",
                );
            }
        }
        self.snapshot.pending = remaining;
        if self.snapshot.acknowledgements.len() + 1 >= self.snapshot.config.quorum_size
            && self.snapshot.fence.is_none()
            && !self.snapshot.committed
        {
            self.snapshot.committed = true;
            self.record(
                SimulationEventKind::QueueCommitted,
                Some(self.snapshot.active_owner_id.clone()),
                None,
                "queue head committed after acknowledgement quorum",
            );
        }
        Ok(())
    }

    fn record(
        &mut self,
        kind: SimulationEventKind,
        from: Option<NodeId>,
        to: Option<NodeId>,
        detail: &str,
    ) {
        if self.snapshot.events.len() >= MAX_EVENTS {
            self.invariant_failures
                .push("simulation event bound exceeded".into());
            return;
        }
        self.snapshot.next_event_sequence = self.snapshot.next_event_sequence.saturating_add(1);
        self.snapshot.events.push(SimulationEvent {
            sequence: self.snapshot.next_event_sequence,
            tick: self.snapshot.tick,
            kind,
            from,
            to,
            detail: detail.to_string(),
        });
    }

    fn safety_state_is_valid(&self) -> bool {
        self.snapshot.owner_term > 0
            && self.snapshot.ownership_epoch > 0
            && self.snapshot.queue_sequence > 0
            && (!self.snapshot.committed
                || (self.snapshot.fence.is_none()
                    && self.snapshot.acknowledgements.len() + 1
                        >= self.snapshot.config.quorum_size))
    }

    fn assert_invariants(&mut self) {
        if self.snapshot.tick > self.snapshot.config.max_ticks {
            self.invariant_failures
                .push("tick exceeds configured maximum".into());
        }
        if self.snapshot.committed && self.snapshot.fence.is_some() {
            self.invariant_failures
                .push("committed queue retains an active fence".into());
        }
        if self.snapshot.acknowledgements.len() >= self.snapshot.config.quorum_size
            && self.snapshot.fence.is_some()
        {
            self.invariant_failures
                .push("fenced queue accumulated a commit quorum".into());
        }
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<(), MultiRegionSimulationError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(MultiRegionSimulationError::InvalidConfiguration(format!(
            "{label} identifier is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_region_configuration_is_deterministic_and_bounded() {
        let first = MultiRegionSimulationConfig::three_region("deterministic", 7).unwrap();
        let second = MultiRegionSimulationConfig::three_region("deterministic", 7).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.nodes.len(), 5);
        assert_eq!(first.quorum_size, 3);
    }
}
