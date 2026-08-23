use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{Display, Formatter};

use crate::semantic::{
    validate_function_with_profile, SemanticDiagnostic, SemanticValidationReport,
    TargetCapabilityProfile,
};
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::walker::{
    DiagnosticSeverity, ExpressionKind, NodeKind, SourceSpan, StatementKind, TypedExpression, Ueg,
    UegDiagnostic,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyGraphError {
    DuplicateFunction {
        name: String,
        first_index: usize,
        duplicate_index: usize,
    },
    FunctionIndexOutOfBounds {
        index: usize,
        function_count: usize,
    },
    FingerprintShapeMismatch {
        fingerprint_functions: usize,
        ueg_functions: usize,
    },
}

impl Display for DependencyGraphError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateFunction {
                name,
                first_index,
                duplicate_index,
            } => write!(
                formatter,
                "function `{name}` is declared at indexes {first_index} and {duplicate_index}"
            ),
            Self::FunctionIndexOutOfBounds {
                index,
                function_count,
            } => write!(
                formatter,
                "changed function index {index} is outside {function_count} functions"
            ),
            Self::FingerprintShapeMismatch {
                fingerprint_functions,
                ueg_functions,
            } => write!(
                formatter,
                "fingerprint describes {fingerprint_functions} functions but UEG has {ueg_functions}"
            ),
        }
    }
}

impl std::error::Error for DependencyGraphError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncrementalValidationError {
    InvalidUeg { diagnostics: Vec<UegDiagnostic> },
    Dependency(DependencyGraphError),
}

impl Display for IncrementalValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUeg { diagnostics } => {
                write!(
                    formatter,
                    "UEG contains {} blocking diagnostics",
                    diagnostics.len()
                )
            }
            Self::Dependency(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for IncrementalValidationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyGraph {
    function_names: Vec<String>,
    dependencies: Vec<BTreeSet<usize>>,
    dependents: Vec<BTreeSet<usize>>,
}

impl DependencyGraph {
    pub fn from_ueg(ueg: &Ueg) -> Result<Self, DependencyGraphError> {
        let mut name_to_index = BTreeMap::new();
        let mut function_names = Vec::with_capacity(ueg.nodes.len());
        for (index, node) in ueg.nodes.iter().enumerate() {
            let NodeKind::Lambda(lambda) = node;
            if let Some(first_index) = name_to_index.insert(lambda.name.clone(), index) {
                return Err(DependencyGraphError::DuplicateFunction {
                    name: lambda.name.clone(),
                    first_index,
                    duplicate_index: index,
                });
            }
            function_names.push(lambda.name.clone());
        }

        let mut dependencies = vec![BTreeSet::new(); function_names.len()];
        let mut dependents = vec![BTreeSet::new(); function_names.len()];
        for (index, node) in ueg.nodes.iter().enumerate() {
            let NodeKind::Lambda(lambda) = node;
            for statement in &lambda.statements {
                collect_statement_dependencies(
                    &statement.kind,
                    &name_to_index,
                    &mut dependencies[index],
                );
            }
            for dependency in dependencies[index].clone() {
                dependents[dependency].insert(index);
            }
        }

        Ok(Self {
            function_names,
            dependencies,
            dependents,
        })
    }

    pub fn function_names(&self) -> &[String] {
        &self.function_names
    }

    pub fn dependencies_for(&self, index: usize) -> Result<&BTreeSet<usize>, DependencyGraphError> {
        self.dependencies
            .get(index)
            .ok_or(DependencyGraphError::FunctionIndexOutOfBounds {
                index,
                function_count: self.function_names.len(),
            })
    }

    pub fn dependents_for(&self, index: usize) -> Result<&BTreeSet<usize>, DependencyGraphError> {
        self.dependents
            .get(index)
            .ok_or(DependencyGraphError::FunctionIndexOutOfBounds {
                index,
                function_count: self.function_names.len(),
            })
    }

    pub fn affected_by_changed(
        &self,
        changed: &BTreeSet<usize>,
    ) -> Result<BTreeSet<usize>, DependencyGraphError> {
        let mut affected = BTreeSet::new();
        let mut queue = VecDeque::new();
        for index in changed {
            if *index >= self.function_names.len() {
                return Err(DependencyGraphError::FunctionIndexOutOfBounds {
                    index: *index,
                    function_count: self.function_names.len(),
                });
            }
            if affected.insert(*index) {
                queue.push_back(*index);
            }
        }
        while let Some(changed_index) = queue.pop_front() {
            for dependent in self.dependents[changed_index].iter().copied() {
                if affected.insert(dependent) {
                    queue.push_back(dependent);
                }
            }
        }
        Ok(affected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalValidationReport {
    pub report: SemanticValidationReport,
    pub changed_functions: BTreeSet<usize>,
    pub affected_functions: BTreeSet<usize>,
    pub revalidated_functions: BTreeSet<usize>,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionReportEntry {
    profile_key: SemanticCacheKey,
    function_key: SemanticCacheKey,
    report: SemanticValidationReport,
}

#[derive(Debug, Clone)]
pub struct DependencyAwareSemanticValidator {
    capacity: usize,
    entries: VecDeque<FunctionReportEntry>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl DependencyAwareSemanticValidator {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::with_capacity(capacity.max(1)),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    pub fn validate(
        &mut self,
        ueg: &Ueg,
        profile: TargetCapabilityProfile,
        fingerprint: &SemanticFingerprint,
        changed_functions: &BTreeSet<usize>,
    ) -> Result<IncrementalValidationReport, IncrementalValidationError> {
        let blocking_diagnostics = ueg
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .cloned()
            .collect::<Vec<_>>();
        if !blocking_diagnostics.is_empty() || ueg.nodes.is_empty() {
            return Err(IncrementalValidationError::InvalidUeg {
                diagnostics: blocking_diagnostics,
            });
        }
        if fingerprint.function_keys().len() != ueg.nodes.len() {
            return Err(IncrementalValidationError::Dependency(
                DependencyGraphError::FingerprintShapeMismatch {
                    fingerprint_functions: fingerprint.function_keys().len(),
                    ueg_functions: ueg.nodes.len(),
                },
            ));
        }
        let graph =
            DependencyGraph::from_ueg(ueg).map_err(IncrementalValidationError::Dependency)?;
        let affected_functions = graph
            .affected_by_changed(changed_functions)
            .map_err(IncrementalValidationError::Dependency)?;
        let mut diagnostics = Vec::new();
        let mut expression_count = 0;
        let mut revalidated_functions = BTreeSet::new();
        let hits_before = self.hits;
        let misses_before = self.misses;
        for function_index in &affected_functions {
            let function_key = fingerprint.function_keys()[*function_index];
            let report = if let Some(report) = self.lookup(fingerprint.profile_key(), function_key)
            {
                report
            } else {
                revalidated_functions.insert(*function_index);
                let report = validate_function_with_profile(ueg, *function_index, profile.clone())
                    .map_err(|_| {
                        IncrementalValidationError::Dependency(
                            DependencyGraphError::FunctionIndexOutOfBounds {
                                index: *function_index,
                                function_count: ueg.nodes.len(),
                            },
                        )
                    })?;
                self.insert(fingerprint.profile_key(), function_key, report.clone());
                report
            };
            expression_count += report.expression_count;
            diagnostics.extend(report.diagnostics);
        }
        diagnostics.sort_by(|left, right| {
            (
                left.span.start_byte,
                left.span.end_byte,
                left.code.as_str(),
                left.message.as_str(),
            )
                .cmp(&(
                    right.span.start_byte,
                    right.span.end_byte,
                    right.code.as_str(),
                    right.message.as_str(),
                ))
        });
        Ok(IncrementalValidationReport {
            report: SemanticValidationReport {
                target: profile.target,
                function_count: ueg.nodes.len(),
                expression_count,
                diagnostics,
            },
            changed_functions: changed_functions.clone(),
            affected_functions,
            revalidated_functions,
            cache_hits: self.hits.saturating_sub(hits_before),
            cache_misses: self.misses.saturating_sub(misses_before),
        })
    }

    pub fn cache_metrics(&self) -> (usize, usize, u64, u64, u64) {
        (
            self.capacity,
            self.entries.len(),
            self.hits,
            self.misses,
            self.evictions,
        )
    }

    fn lookup(
        &mut self,
        profile_key: SemanticCacheKey,
        function_key: SemanticCacheKey,
    ) -> Option<SemanticValidationReport> {
        let position = self.entries.iter().position(|entry| {
            entry.profile_key == profile_key && entry.function_key == function_key
        });
        match position {
            Some(position) => {
                let entry = self
                    .entries
                    .remove(position)
                    .expect("cache position exists");
                let report = entry.report.clone();
                self.entries.push_back(entry);
                self.hits += 1;
                Some(report)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    fn insert(
        &mut self,
        profile_key: SemanticCacheKey,
        function_key: SemanticCacheKey,
        report: SemanticValidationReport,
    ) {
        if let Some(position) = self.entries.iter().position(|entry| {
            entry.profile_key == profile_key && entry.function_key == function_key
        }) {
            self.entries.remove(position);
        } else if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.evictions += 1;
        }
        self.entries.push_back(FunctionReportEntry {
            profile_key,
            function_key,
            report,
        });
    }
}

pub fn validate_changed_functions(
    ueg: &Ueg,
    profile: TargetCapabilityProfile,
    changed_functions: &BTreeSet<usize>,
) -> Result<IncrementalValidationReport, IncrementalValidationError> {
    let blocking_diagnostics = ueg
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .cloned()
        .collect::<Vec<_>>();
    if !blocking_diagnostics.is_empty() || ueg.nodes.is_empty() {
        return Err(IncrementalValidationError::InvalidUeg {
            diagnostics: blocking_diagnostics,
        });
    }

    let graph = DependencyGraph::from_ueg(ueg).map_err(IncrementalValidationError::Dependency)?;
    let affected_functions = graph
        .affected_by_changed(changed_functions)
        .map_err(IncrementalValidationError::Dependency)?;
    let mut diagnostics = Vec::new();
    let mut expression_count = 0;
    for function_index in &affected_functions {
        let function_report = validate_function_with_profile(ueg, *function_index, profile.clone())
            .map_err(|_| {
                IncrementalValidationError::Dependency(
                    DependencyGraphError::FunctionIndexOutOfBounds {
                        index: *function_index,
                        function_count: ueg.nodes.len(),
                    },
                )
            })?;
        expression_count += function_report.expression_count;
        diagnostics.extend(function_report.diagnostics);
    }
    diagnostics.sort_by(|left, right| {
        (
            left.span.start_byte,
            left.span.end_byte,
            left.code.as_str(),
            left.message.as_str(),
        )
            .cmp(&(
                right.span.start_byte,
                right.span.end_byte,
                right.code.as_str(),
                right.message.as_str(),
            ))
    });
    let cache_misses = affected_functions.len() as u64;

    Ok(IncrementalValidationReport {
        report: SemanticValidationReport {
            target: profile.target,
            function_count: ueg.nodes.len(),
            expression_count,
            diagnostics,
        },
        changed_functions: changed_functions.clone(),
        affected_functions,
        revalidated_functions: changed_functions.clone(),
        cache_hits: 0,
        cache_misses,
    })
}

fn collect_statement_dependencies(
    statement: &StatementKind,
    names: &BTreeMap<String, usize>,
    output: &mut BTreeSet<usize>,
) {
    match statement {
        StatementKind::If { condition }
        | StatementKind::Return {
            expression: condition,
        }
        | StatementKind::Print {
            expression: condition,
        } => collect_expression_dependencies(condition, names, output),
        StatementKind::Assign { target, value } => {
            collect_expression_dependencies(target, names, output);
            collect_expression_dependencies(value, names, output);
        }
        StatementKind::TupleAssign { targets, values } => {
            for expression in targets.iter().chain(values) {
                collect_expression_dependencies(expression, names, output);
            }
        }
        StatementKind::RangeLoop {
            target, start, end, ..
        } => {
            collect_expression_dependencies(target, names, output);
            collect_expression_dependencies(start, names, output);
            collect_expression_dependencies(end, names, output);
        }
        StatementKind::Unsupported { .. } => {}
    }
}

fn collect_expression_dependencies(
    expression: &TypedExpression,
    names: &BTreeMap<String, usize>,
    output: &mut BTreeSet<usize>,
) {
    if let ExpressionKind::Identifier { name } = &expression.kind {
        if let Some(index) = names.get(name) {
            output.insert(*index);
        }
    }
    match &expression.kind {
        ExpressionKind::Unary { operand, .. } => {
            collect_expression_dependencies(operand, names, output)
        }
        ExpressionKind::Binary { left, right, .. } => {
            collect_expression_dependencies(left, names, output);
            collect_expression_dependencies(right, names, output);
        }
        ExpressionKind::Call {
            function,
            arguments,
        } => {
            collect_expression_dependencies(function, names, output);
            for argument in arguments {
                collect_expression_dependencies(argument, names, output);
            }
        }
        ExpressionKind::Tuple { items } => {
            for item in items {
                collect_expression_dependencies(item, names, output);
            }
        }
        ExpressionKind::Identifier { .. }
        | ExpressionKind::Integer { .. }
        | ExpressionKind::Float { .. }
        | ExpressionKind::String { .. }
        | ExpressionKind::Boolean { .. }
        | ExpressionKind::Unsupported { .. } => {}
    }
}

#[allow(dead_code)]
fn _typed_diagnostics_are_exhaustive(_diagnostic: &SemanticDiagnostic, _span: &SourceSpan) {}
