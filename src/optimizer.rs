use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{Display, Formatter};

use crate::walker::{DiagnosticSeverity, NodeKind, StatementKind, Ueg};

pub trait OptimizerHook: Send + Sync {
    fn before_optimize(&self, ueg: &Ueg) -> Result<(), String>;
    fn after_optimize(&self, ueg: &Ueg) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptimizerError {
    InvalidUeg {
        diagnostic_count: usize,
    },
    UnknownRoot {
        name: String,
    },
    DuplicateFunction {
        name: String,
    },
    HookRejected {
        phase: &'static str,
        message: String,
    },
}

impl Display for OptimizerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUeg { diagnostic_count } => {
                write!(
                    formatter,
                    "cannot optimize invalid UEG with {diagnostic_count} errors"
                )
            }
            Self::UnknownRoot { name } => {
                write!(formatter, "optimizer root does not exist: {name}")
            }
            Self::DuplicateFunction { name } => {
                write!(formatter, "UEG contains duplicate function name: {name}")
            }
            Self::HookRejected { phase, message } => {
                write!(formatter, "optimizer {phase} hook rejected: {message}")
            }
        }
    }
}

impl std::error::Error for OptimizerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizationStats {
    pub before_nodes: usize,
    pub after_nodes: usize,
    pub removed_nodes: usize,
    pub roots: Vec<String>,
    pub removed_functions: Vec<String>,
}

pub struct OptimizerPipeline {
    roots: Option<BTreeSet<String>>,
    hooks: Vec<Box<dyn OptimizerHook>>,
}

impl Default for OptimizerPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizerPipeline {
    pub fn new() -> Self {
        Self {
            roots: None,
            hooks: Vec::new(),
        }
    }

    pub fn with_roots<I, S>(roots: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            roots: Some(roots.into_iter().map(Into::into).collect()),
            hooks: Vec::new(),
        }
    }

    pub fn add_hook<H>(&mut self, hook: H)
    where
        H: OptimizerHook + 'static,
    {
        self.hooks.push(Box::new(hook));
    }

    pub fn optimize(&self, ueg: &Ueg) -> Result<(Ueg, OptimizationStats), OptimizerError> {
        validate(ueg)?;
        for hook in &self.hooks {
            hook.before_optimize(ueg)
                .map_err(|message| OptimizerError::HookRejected {
                    phase: "before",
                    message,
                })?;
        }

        let before_nodes = ueg.nodes.len();
        let index = function_index(ueg)?;
        let mut optimized = ueg.clone();
        let mut removed_functions = Vec::new();
        let roots = self
            .roots
            .as_ref()
            .map(|roots| roots.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        if let Some(roots) = &self.roots {
            for root in roots {
                if !index.contains_key(root) {
                    return Err(OptimizerError::UnknownRoot { name: root.clone() });
                }
            }
            let reachable = reachable_functions(ueg, &index, roots);
            optimized.nodes.retain(|node| {
                let name = node_name(node);
                if reachable.contains(name) {
                    true
                } else {
                    removed_functions.push(name.to_string());
                    false
                }
            });
        }

        let stats = OptimizationStats {
            before_nodes,
            after_nodes: optimized.nodes.len(),
            removed_nodes: before_nodes.saturating_sub(optimized.nodes.len()),
            roots,
            removed_functions,
        };
        for hook in &self.hooks {
            hook.after_optimize(&optimized)
                .map_err(|message| OptimizerError::HookRejected {
                    phase: "after",
                    message,
                })?;
        }
        Ok((optimized, stats))
    }
}

fn validate(ueg: &Ueg) -> Result<(), OptimizerError> {
    if ueg.validate() {
        return Ok(());
    }
    let diagnostic_count = ueg
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .count();
    Err(OptimizerError::InvalidUeg { diagnostic_count })
}

fn function_index(ueg: &Ueg) -> Result<BTreeMap<String, usize>, OptimizerError> {
    let mut index = BTreeMap::new();
    for (position, node) in ueg.nodes.iter().enumerate() {
        let name = node_name(node).to_string();
        if index.insert(name.clone(), position).is_some() {
            return Err(OptimizerError::DuplicateFunction { name });
        }
    }
    Ok(index)
}

fn reachable_functions(
    ueg: &Ueg,
    index: &BTreeMap<String, usize>,
    roots: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending = VecDeque::from_iter(roots.iter().cloned());
    while let Some(name) = pending.pop_front() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(&position) = index.get(&name) else {
            continue;
        };
        let NodeKind::Lambda(lambda) = &ueg.nodes[position];
        for candidate in referenced_names(lambda) {
            if index.contains_key(&candidate) && !reachable.contains(&candidate) {
                pending.push_back(candidate);
            }
        }
    }
    reachable
}

fn referenced_names(lambda: &crate::walker::LambdaNode) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for statement in &lambda.statements {
        match &statement.kind {
            StatementKind::If { condition } => names.extend(identifier_tokens(&condition.source)),
            StatementKind::Return { expression } => {
                names.extend(identifier_tokens(&expression.source))
            }
            StatementKind::TupleAssign { values, .. } => {
                for value in values {
                    names.extend(identifier_tokens(&value.source));
                }
            }
            StatementKind::RangeLoop { start, end, .. } => {
                names.extend(identifier_tokens(&start.source));
                names.extend(identifier_tokens(&end.source));
            }
            StatementKind::Print { expression } => {
                names.extend(identifier_tokens(&expression.source))
            }
            StatementKind::Unsupported { source } => names.extend(identifier_tokens(source)),
        }
    }
    names
}

fn identifier_tokens(source: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    let mut current = String::new();
    for character in source.chars() {
        if character == '_' || character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            tokens.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.insert(current);
    }
    tokens
}

fn node_name(node: &NodeKind) -> &str {
    match node {
        NodeKind::Lambda(lambda) => &lambda.name,
    }
}
