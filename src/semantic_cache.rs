use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::codegen::TargetBinding;
use crate::semantic::{
    validate_ueg_with_profile, SemanticValidationReport, TargetCapabilityProfile,
};
use crate::walker::{
    AstFragment, BinaryOperator, DiagnosticSeverity, ExpressionKind, LambdaNode, NodeKind,
    SourceSpan, StatementKind, TypedExpression, TypedParameter, TypedStatement, Ueg, UnaryOperator,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticFingerprintError {
    FunctionIndexOutOfBounds { index: usize, function_count: usize },
}

impl Display for SemanticFingerprintError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FunctionIndexOutOfBounds {
                index,
                function_count,
            } => write!(
                formatter,
                "function fingerprint index {index} is outside {function_count} functions"
            ),
        }
    }
}

impl std::error::Error for SemanticFingerprintError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticCacheKey([u8; 32]);

impl SemanticCacheKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticFingerprint {
    profile_key: SemanticCacheKey,
    function_keys: Vec<SemanticCacheKey>,
    root_key: SemanticCacheKey,
}

impl SemanticFingerprint {
    pub fn from_ueg(ueg: &Ueg, profile: &TargetCapabilityProfile) -> Self {
        let profile_key = profile_fingerprint(profile);
        let function_keys = ueg
            .nodes
            .iter()
            .map(|node| {
                let NodeKind::Lambda(lambda) = node;
                function_fingerprint(lambda)
            })
            .collect::<Vec<_>>();
        let root_key = compose_root_key(profile_key, &function_keys);
        Self {
            profile_key,
            function_keys,
            root_key,
        }
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn root_key(&self) -> SemanticCacheKey {
        self.root_key
    }

    pub fn function_keys(&self) -> &[SemanticCacheKey] {
        &self.function_keys
    }

    pub fn replace_function(
        &mut self,
        index: usize,
        lambda: &LambdaNode,
    ) -> Result<(), SemanticFingerprintError> {
        let Some(function_key) = self.function_keys.get_mut(index) else {
            return Err(SemanticFingerprintError::FunctionIndexOutOfBounds {
                index,
                function_count: self.function_keys.len(),
            });
        };
        *function_key = function_fingerprint(lambda);
        self.root_key = compose_root_key(self.profile_key, &self.function_keys);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
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

    pub fn fingerprint_for(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> SemanticFingerprint {
        SemanticFingerprint::from_ueg(ueg, profile)
    }

    pub fn key_for(&self, ueg: &Ueg, profile: &TargetCapabilityProfile) -> SemanticCacheKey {
        self.fingerprint_for(ueg, profile).root_key()
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
        let fingerprint = self.fingerprint_for(ueg, profile);
        self.validate_with_fingerprint(ueg, profile, &fingerprint)
    }

    pub fn validate_with_fingerprint(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        fingerprint: &SemanticFingerprint,
    ) -> SemanticValidationReport {
        self.validate_with_key(ueg, profile, fingerprint.root_key())
    }

    pub fn validate_with_key(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        key: SemanticCacheKey,
    ) -> SemanticValidationReport {
        // Hold the state lock across the miss computation so concurrent identical
        // validations cannot all observe a miss and inflate the evidence metrics.
        // The semantic validator is local and does not call back into this cache.
        let mut state = self.lock_state();
        if let Some(position) = state
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key)
        {
            let entry = state
                .entries
                .remove(position)
                .expect("cache position exists");
            let report = entry.1.clone();
            state.entries.push_back(entry);
            state.hits += 1;
            return report;
        }

        state.misses += 1;
        let report = validate_ueg_with_profile(ueg, profile.clone());
        if state.entries.len() == self.capacity {
            state.entries.pop_front();
            state.evictions += 1;
        }
        state.entries.push_back((key, report.clone()));
        state.insertions += 1;
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

fn profile_fingerprint(profile: &TargetCapabilityProfile) -> SemanticCacheKey {
    let mut hasher = Sha256::new();
    feed_str(&mut hasher, "un1c0-semantic-profile-v2");
    feed_str(&mut hasher, profile.target.label());
    feed_bool(&mut hasher, profile.supports_calls);
    feed_bool(&mut hasher, profile.supports_tuples);
    feed_bool(&mut hasher, profile.supports_strings);
    feed_bool(&mut hasher, profile.supports_booleans);
    feed_bool(&mut hasher, profile.supports_floats);
    feed_unary_operators(&mut hasher, &profile.supported_unary_operators);
    feed_binary_operators(&mut hasher, &profile.supported_binary_operators);
    SemanticCacheKey(hasher.finalize().into())
}

fn function_fingerprint(lambda: &LambdaNode) -> SemanticCacheKey {
    let mut hasher = Sha256::new();
    feed_str(&mut hasher, "un1c0-semantic-function-v2");
    feed_str(&mut hasher, &lambda.name);
    feed_parameters(&mut hasher, &lambda.ast_fragment.params);
    feed_option_str(&mut hasher, lambda.ast_fragment.ret.as_deref());
    feed_span(&mut hasher, &lambda.ast_fragment.source_span);
    feed_u64(&mut hasher, lambda.ast_fragment.statements.len() as u64);
    for statement in &lambda.ast_fragment.statements {
        feed_statement(&mut hasher, statement);
    }
    SemanticCacheKey(hasher.finalize().into())
}

fn compose_root_key(
    profile_key: SemanticCacheKey,
    function_keys: &[SemanticCacheKey],
) -> SemanticCacheKey {
    let mut hasher = Sha256::new();
    feed_str(&mut hasher, "un1c0-semantic-root-v2");
    feed_key(&mut hasher, profile_key);
    feed_u64(&mut hasher, function_keys.len() as u64);
    for function_key in function_keys {
        feed_key(&mut hasher, *function_key);
    }
    SemanticCacheKey(hasher.finalize().into())
}

fn feed_parameters(hasher: &mut Sha256, parameters: &[TypedParameter]) {
    feed_u64(hasher, parameters.len() as u64);
    for parameter in parameters {
        feed_str(hasher, &parameter.name);
        feed_str(hasher, &parameter.annotation);
    }
}

fn feed_statement(hasher: &mut Sha256, statement: &TypedStatement) {
    feed_span(hasher, &statement.span);
    match &statement.kind {
        StatementKind::If { condition } => {
            feed_u64(hasher, 1);
            feed_expression(hasher, condition);
        }
        StatementKind::Return { expression } => {
            feed_u64(hasher, 2);
            feed_expression(hasher, expression);
        }
        StatementKind::Assign { target, value } => {
            feed_u64(hasher, 3);
            feed_expression(hasher, target);
            feed_expression(hasher, value);
        }
        StatementKind::TupleAssign { targets, values } => {
            feed_u64(hasher, 4);
            feed_expressions(hasher, targets);
            feed_expressions(hasher, values);
        }
        StatementKind::RangeLoop {
            target,
            start,
            end,
            inclusive,
        } => {
            feed_u64(hasher, 5);
            feed_expression(hasher, target);
            feed_expression(hasher, start);
            feed_expression(hasher, end);
            feed_bool(hasher, *inclusive);
        }
        StatementKind::Print { expression } => {
            feed_u64(hasher, 6);
            feed_expression(hasher, expression);
        }
        StatementKind::Unsupported { source } => {
            feed_u64(hasher, 7);
            feed_str(hasher, source);
        }
    }
}

fn feed_expressions(hasher: &mut Sha256, expressions: &[TypedExpression]) {
    feed_u64(hasher, expressions.len() as u64);
    for expression in expressions {
        feed_expression(hasher, expression);
    }
}

fn feed_expression(hasher: &mut Sha256, expression: &TypedExpression) {
    feed_span(hasher, &expression.span);
    feed_str(hasher, &expression.source);
    match &expression.kind {
        ExpressionKind::Identifier { name } => {
            feed_u64(hasher, 1);
            feed_str(hasher, name);
        }
        ExpressionKind::Integer { value } => {
            feed_u64(hasher, 2);
            hasher.update(value.to_le_bytes());
        }
        ExpressionKind::Float { value } => {
            feed_u64(hasher, 3);
            feed_str(hasher, value);
        }
        ExpressionKind::String { value } => {
            feed_u64(hasher, 4);
            feed_str(hasher, value);
        }
        ExpressionKind::Boolean { value } => {
            feed_u64(hasher, 5);
            feed_bool(hasher, *value);
        }
        ExpressionKind::Unary { operator, operand } => {
            feed_u64(hasher, 6);
            feed_u64(hasher, unary_code(operator));
            feed_expression(hasher, operand);
        }
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            feed_u64(hasher, 7);
            feed_u64(hasher, binary_code(operator));
            feed_expression(hasher, left);
            feed_expression(hasher, right);
        }
        ExpressionKind::Call {
            function,
            arguments,
        } => {
            feed_u64(hasher, 8);
            feed_expression(hasher, function);
            feed_expressions(hasher, arguments);
        }
        ExpressionKind::Tuple { items } => {
            feed_u64(hasher, 9);
            feed_expressions(hasher, items);
        }
        ExpressionKind::Unsupported { source } => {
            feed_u64(hasher, 10);
            feed_str(hasher, source);
        }
    }
}

fn feed_span(hasher: &mut Sha256, span: &SourceSpan) {
    feed_u64(hasher, span.start_byte as u64);
    feed_u64(hasher, span.end_byte as u64);
    feed_u64(hasher, span.start_line as u64);
    feed_u64(hasher, span.start_column as u64);
    feed_u64(hasher, span.end_line as u64);
    feed_u64(hasher, span.end_column as u64);
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

fn feed_option_str(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            feed_bool(hasher, true);
            feed_str(hasher, value);
        }
        None => feed_bool(hasher, false),
    }
}

fn feed_key(hasher: &mut Sha256, key: SemanticCacheKey) {
    hasher.update(key.0);
}

fn feed_bytes(hasher: &mut Sha256, value: &[u8]) {
    feed_u64(hasher, value.len() as u64);
    hasher.update(value);
}

#[allow(dead_code)]
fn _typed_contracts_are_exhaustive(_fragment: &AstFragment, _severity: &DiagnosticSeverity) {}
