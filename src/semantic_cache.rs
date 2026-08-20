use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::codegen::TargetBinding;
use crate::semantic::{
    validate_ueg_with_profile, SemanticValidationReport, TargetCapabilityProfile,
};
use crate::walker::{
    AstFragment, BinaryOperator, ExpressionKind, SourceSpan, StatementKind, Ueg, UnaryOperator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticCacheConfigError {
    ZeroCapacity,
}

impl Display for SemanticCacheConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroCapacity => {
                formatter.write_str("semantic cache capacity must be greater than zero")
            }
        }
    }
}

impl std::error::Error for SemanticCacheConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticCacheKey([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SemanticCacheMetrics {
    pub capacity: usize,
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
}

#[derive(Debug, Clone)]
struct CacheState {
    entries: VecDeque<(SemanticCacheKey, SemanticValidationReport)>,
    hits: u64,
    misses: u64,
    insertions: u64,
    evictions: u64,
}

#[derive(Debug, Clone)]
pub struct SemanticValidationCache {
    capacity: usize,
    state: Arc<Mutex<CacheState>>,
}

impl SemanticValidationCache {
    pub fn new(capacity: usize) -> Result<Self, SemanticCacheConfigError> {
        if capacity == 0 {
            return Err(SemanticCacheConfigError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            state: Arc::new(Mutex::new(CacheState {
                entries: VecDeque::with_capacity(capacity),
                hits: 0,
                misses: 0,
                insertions: 0,
                evictions: 0,
            })),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.lock_state().entries.len()
    }

    pub fn key_for(&self, ueg: &Ueg, profile: &TargetCapabilityProfile) -> SemanticCacheKey {
        fingerprint(ueg, profile)
    }

    pub fn validate_for_target(
        &self,
        ueg: &Ueg,
        target: TargetBinding,
    ) -> SemanticValidationReport {
        let profile = TargetCapabilityProfile::for_target(target);
        self.validate_with_profile(ueg, &profile)
    }

    pub fn validate_with_profile(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> SemanticValidationReport {
        let key = self.key_for(ueg, profile);
        self.validate_with_key(ueg, profile, key)
    }

    pub fn validate_with_key(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        key: SemanticCacheKey,
    ) -> SemanticValidationReport {
        if let Some(report) = self.lookup(key) {
            return report;
        }
        let report = validate_ueg_with_profile(ueg, profile.clone());
        self.insert(key, report.clone());
        report
    }

    pub fn metrics(&self) -> SemanticCacheMetrics {
        let state = self.lock_state();
        SemanticCacheMetrics {
            capacity: self.capacity,
            entries: state.entries.len(),
            hits: state.hits,
            misses: state.misses,
            insertions: state.insertions,
            evictions: state.evictions,
        }
    }

    fn lookup(&self, key: SemanticCacheKey) -> Option<SemanticValidationReport> {
        let mut state = self.lock_state();
        let position = state
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key);
        match position {
            Some(position) => {
                let entry = state
                    .entries
                    .remove(position)
                    .expect("cache position exists");
                let report = entry.1.clone();
                state.entries.push_back(entry);
                state.hits += 1;
                Some(report)
            }
            None => {
                state.misses += 1;
                None
            }
        }
    }

    fn insert(&self, key: SemanticCacheKey, report: SemanticValidationReport) {
        let mut state = self.lock_state();
        if let Some(position) = state
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key)
        {
            state.entries.remove(position);
        } else if state.entries.len() == self.capacity {
            state.entries.pop_front();
            state.evictions += 1;
        }
        state.entries.push_back((key, report));
        state.insertions += 1;
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn fingerprint(ueg: &Ueg, profile: &TargetCapabilityProfile) -> SemanticCacheKey {
    let mut hasher = Sha256::new();
    feed_str(&mut hasher, "un1c0-semantic-cache-v1");
    feed_str(&mut hasher, profile.target.label());
    feed_bool(&mut hasher, profile.supports_calls);
    feed_bool(&mut hasher, profile.supports_tuples);
    feed_bool(&mut hasher, profile.supports_strings);
    feed_bool(&mut hasher, profile.supports_booleans);
    feed_bool(&mut hasher, profile.supports_floats);
    feed_unary_operators(&mut hasher, &profile.supported_unary_operators);
    feed_binary_operators(&mut hasher, &profile.supported_binary_operators);
    feed_u64(&mut hasher, ueg.nodes.len() as u64);
    for node in &ueg.nodes {
        let crate::walker::NodeKind::Lambda(lambda) = node;
        let bytes = serde_json::to_vec(&lambda.ast_fragment).expect("typed AST is serializable");
        feed_bytes(&mut hasher, &bytes);
    }
    SemanticCacheKey(hasher.finalize().into())
}

fn feed_unary_operators(hasher: &mut Sha256, operators: &[UnaryOperator]) {
    feed_u64(hasher, operators.len() as u64);
    for operator in operators {
        feed_u64(hasher, unary_code(operator));
    }
}

fn feed_binary_operators(hasher: &mut Sha256, operators: &[BinaryOperator]) {
    feed_u64(hasher, operators.len() as u64);
    for operator in operators {
        feed_u64(hasher, binary_code(operator));
    }
}

fn unary_code(operator: &UnaryOperator) -> u64 {
    match operator {
        UnaryOperator::Not => 1,
        UnaryOperator::Negate => 2,
        UnaryOperator::Positive => 3,
    }
}

fn binary_code(operator: &BinaryOperator) -> u64 {
    match operator {
        BinaryOperator::Add => 1,
        BinaryOperator::Subtract => 2,
        BinaryOperator::Multiply => 3,
        BinaryOperator::Divide => 4,
        BinaryOperator::Modulo => 5,
        BinaryOperator::Equal => 6,
        BinaryOperator::NotEqual => 7,
        BinaryOperator::Less => 8,
        BinaryOperator::LessEqual => 9,
        BinaryOperator::Greater => 10,
        BinaryOperator::GreaterEqual => 11,
        BinaryOperator::And => 12,
        BinaryOperator::Or => 13,
    }
}

fn feed_bool(hasher: &mut Sha256, value: bool) {
    feed_u64(hasher, u64::from(value));
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

#[allow(dead_code)]
fn _typed_contracts_are_exhaustive(
    _fragment: &AstFragment,
    _span: &SourceSpan,
    _expression: &ExpressionKind,
    _statement: &StatementKind,
) {
}
