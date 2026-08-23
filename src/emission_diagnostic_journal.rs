use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

pub const MAX_DIAGNOSTIC_JOURNAL_ENTRIES: usize = 4096;
const JOURNAL_DOMAIN: &[u8] = b"un1c0/phase78/diagnostic-observation-journal/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticJournalError {
    InvalidCapacity,
    InvalidNodeId,
    InvalidConnectionId,
    InvalidSequence,
    Full { entries: usize, maximum: usize },
    InvalidCheckpoint,
}

impl Display for DiagnosticJournalError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCapacity => {
                formatter.write_str("diagnostic journal capacity is outside the safe range")
            }
            Self::InvalidNodeId => {
                formatter.write_str("diagnostic journal node ID must be non-zero")
            }
            Self::InvalidConnectionId => {
                formatter.write_str("diagnostic journal connection ID must be non-zero")
            }
            Self::InvalidSequence => {
                formatter.write_str("diagnostic journal source sequence must be non-zero")
            }
            Self::Full { entries, maximum } => write!(
                formatter,
                "diagnostic journal has {entries} entries; maximum is {maximum}"
            ),
            Self::InvalidCheckpoint => {
                formatter.write_str("diagnostic journal checkpoint is invalid")
            }
        }
    }
}

impl std::error::Error for DiagnosticJournalError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticJournalEntry {
    sequence: u64,
    node_id: u64,
    connection_id: u64,
    source_sequence: u64,
    event_digest: [u8; 32],
    previous_digest: [u8; 32],
}

impl DiagnosticJournalEntry {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn node_id(&self) -> u64 {
        self.node_id
    }

    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    pub fn event_digest(&self) -> [u8; 32] {
        self.event_digest
    }

    pub fn previous_digest(&self) -> [u8; 32] {
        self.previous_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticObservationJournal {
    entries: Vec<DiagnosticJournalEntry>,
    maximum: usize,
}

impl DiagnosticObservationJournal {
    pub fn new(maximum: usize) -> Result<Self, DiagnosticJournalError> {
        if maximum == 0 || maximum > MAX_DIAGNOSTIC_JOURNAL_ENTRIES {
            return Err(DiagnosticJournalError::InvalidCapacity);
        }
        Ok(Self {
            entries: Vec::new(),
            maximum,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn maximum(&self) -> usize {
        self.maximum
    }

    pub fn entries(&self) -> &[DiagnosticJournalEntry] {
        &self.entries
    }

    pub fn checkpoint(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn rollback_to(&mut self, checkpoint: usize) -> Result<(), DiagnosticJournalError> {
        if checkpoint > self.entries.len() {
            return Err(DiagnosticJournalError::InvalidCheckpoint);
        }
        self.entries.truncate(checkpoint);
        Ok(())
    }

    pub fn append(
        &mut self,
        node_id: u64,
        connection_id: u64,
        source_sequence: u64,
        stream_digest: [u8; 32],
    ) -> Result<u64, DiagnosticJournalError> {
        if node_id == 0 {
            return Err(DiagnosticJournalError::InvalidNodeId);
        }
        if connection_id == 0 {
            return Err(DiagnosticJournalError::InvalidConnectionId);
        }
        if source_sequence == 0 {
            return Err(DiagnosticJournalError::InvalidSequence);
        }
        if self.entries.len() >= self.maximum {
            return Err(DiagnosticJournalError::Full {
                entries: self.entries.len(),
                maximum: self.maximum,
            });
        }
        let sequence = self.entries.len() as u64 + 1;
        let previous_digest = self
            .entries
            .last()
            .map_or([0; 32], DiagnosticJournalEntry::event_digest);
        let event_digest = digest_event(
            sequence,
            node_id,
            connection_id,
            source_sequence,
            stream_digest,
            previous_digest,
        );
        self.entries.push(DiagnosticJournalEntry {
            sequence,
            node_id,
            connection_id,
            source_sequence,
            event_digest,
            previous_digest,
        });
        Ok(sequence)
    }

    pub fn verify_integrity(&self) -> bool {
        let mut previous_digest = [0; 32];
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.sequence != index as u64 + 1 || entry.previous_digest != previous_digest {
                return false;
            }
            previous_digest = entry.event_digest;
        }
        true
    }
}

fn digest_event(
    sequence: u64,
    node_id: u64,
    connection_id: u64,
    source_sequence: u64,
    stream_digest: [u8; 32],
    previous_digest: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(JOURNAL_DOMAIN);
    hasher.update(sequence.to_be_bytes());
    hasher.update(node_id.to_be_bytes());
    hasher.update(connection_id.to_be_bytes());
    hasher.update(source_sequence.to_be_bytes());
    hasher.update(stream_digest);
    hasher.update(previous_digest);
    hasher.finalize().into()
}
