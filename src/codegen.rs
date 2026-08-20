use std::fmt::{Display, Formatter};

use crate::lock_free_buffer_pool::{LockFreeBufferPool, PooledBuffer};
use crate::targets::{lower_to_go, lower_to_zig};
use crate::ueg_python::lower_to_python;
use crate::walker::{lower_to_rust, NodeKind, Ueg};

const GO_PREAMBLE: &str = "package main\n\nimport \"fmt\"\n\n";
const ZIG_PREAMBLE: &str = "const std = @import(\"std\");\n\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetBinding {
    Rust,
    Go,
    Zig,
    Python,
}

impl TargetBinding {
    pub const ALL: [Self; 4] = [Self::Rust, Self::Go, Self::Zig, Self::Python];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Zig => "zig",
            Self::Python => "python",
        }
    }

    pub fn preamble(self) -> &'static str {
        match self {
            Self::Rust | Self::Python => "",
            Self::Go => GO_PREAMBLE,
            Self::Zig => ZIG_PREAMBLE,
        }
    }

    fn render_node(self, node: &NodeKind) -> Result<String, GenerationError> {
        let singleton = Ueg {
            nodes: vec![node.clone()],
            diagnostics: Vec::new(),
        };
        match self {
            Self::Rust => Ok(lower_to_rust(&singleton)),
            Self::Go => strip_preamble(lower_to_go(&singleton), GO_PREAMBLE, self),
            Self::Zig => strip_preamble(lower_to_zig(&singleton), ZIG_PREAMBLE, self),
            Self::Python => render_python_node(&singleton),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedChunk {
    pub target: TargetBinding,
    pub node_index: usize,
    pub function_name: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationStats {
    pub target: TargetBinding,
    pub chunks_emitted: usize,
    pub bytes_emitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationError {
    InvalidUeg { diagnostic_count: usize },
    CursorRewind { cursor: usize, node_count: usize },
    EmitterOutput { target: TargetBinding },
    Sink { message: String },
}

impl Display for GenerationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUeg { diagnostic_count } => write!(
                formatter,
                "cannot generate invalid UEG with {diagnostic_count} error diagnostics"
            ),
            Self::CursorRewind { cursor, node_count } => write!(
                formatter,
                "incremental cursor {cursor} is past UEG node count {node_count}"
            ),
            Self::EmitterOutput { target } => {
                write!(
                    formatter,
                    "{} emitter produced an invalid preamble",
                    target.label()
                )
            }
            Self::Sink { message } => {
                write!(formatter, "generation sink rejected a chunk: {message}")
            }
        }
    }
}

impl std::error::Error for GenerationError {}

#[derive(Debug, Clone)]
pub struct IncrementalCodeGenerator {
    target: TargetBinding,
    next_node_index: usize,
}

impl IncrementalCodeGenerator {
    pub fn new(target: TargetBinding) -> Self {
        Self {
            target,
            next_node_index: 0,
        }
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn cursor(&self) -> usize {
        self.next_node_index
    }

    pub fn next_chunk(&mut self, ueg: &Ueg) -> Result<Option<GeneratedChunk>, GenerationError> {
        validate_generation_input(ueg)?;
        if self.next_node_index > ueg.nodes.len() {
            return Err(GenerationError::CursorRewind {
                cursor: self.next_node_index,
                node_count: ueg.nodes.len(),
            });
        }
        let node_index = self.next_node_index;
        let Some(node) = ueg.nodes.get(node_index) else {
            return Ok(None);
        };
        self.next_node_index += 1;
        let function_name = node_name(node).to_string();
        let code = self.target.render_node(node)?;
        Ok(Some(GeneratedChunk {
            target: self.target,
            node_index,
            function_name,
            code,
        }))
    }

    pub fn emit_remaining<F, E>(
        &mut self,
        ueg: &Ueg,
        mut sink: F,
    ) -> Result<GenerationStats, GenerationError>
    where
        F: FnMut(GeneratedChunk) -> Result<(), E>,
        E: Display,
    {
        validate_generation_input(ueg)?;
        let mut stats = GenerationStats {
            target: self.target,
            chunks_emitted: 0,
            bytes_emitted: 0,
        };
        while let Some(chunk) = self.next_chunk(ueg)? {
            let bytes = chunk.code.len();
            sink(chunk).map_err(|error| GenerationError::Sink {
                message: error.to_string(),
            })?;
            stats.chunks_emitted += 1;
            stats.bytes_emitted += bytes;
        }
        Ok(stats)
    }

    pub fn emit_to_string(
        &mut self,
        ueg: &Ueg,
    ) -> Result<(String, GenerationStats), GenerationError> {
        let mut output = self.target.preamble().to_string();
        let stats = self.emit_remaining(ueg, |chunk| {
            output.push_str(&chunk.code);
            Ok::<(), std::convert::Infallible>(())
        })?;
        Ok((output, stats))
    }

    pub fn emit_to_pooled_buffer(
        &mut self,
        ueg: &Ueg,
        pool: &LockFreeBufferPool,
    ) -> Result<(PooledBuffer, GenerationStats), GenerationError> {
        let mut output = pool.checkout();
        output.extend_from_slice(self.target.preamble().as_bytes());
        let stats = self.emit_remaining(ueg, |chunk| {
            output.extend_from_slice(chunk.code.as_bytes());
            Ok::<(), std::convert::Infallible>(())
        })?;
        Ok((output, stats))
    }
}

pub fn generate_incrementally_with_pool(
    ueg: &Ueg,
    target: TargetBinding,
    pool: &LockFreeBufferPool,
) -> Result<(PooledBuffer, GenerationStats), GenerationError> {
    IncrementalCodeGenerator::new(target).emit_to_pooled_buffer(ueg, pool)
}

pub fn generate_incrementally(
    ueg: &Ueg,
    target: TargetBinding,
) -> Result<(String, GenerationStats), GenerationError> {
    IncrementalCodeGenerator::new(target).emit_to_string(ueg)
}

fn validate_generation_input(ueg: &Ueg) -> Result<(), GenerationError> {
    if ueg.validate() {
        return Ok(());
    }
    let diagnostic_count = ueg
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.severity,
                crate::walker::DiagnosticSeverity::Error
            )
        })
        .count();
    Err(GenerationError::InvalidUeg { diagnostic_count })
}

fn node_name(node: &NodeKind) -> &str {
    match node {
        NodeKind::Lambda(lambda) => &lambda.name,
    }
}

fn strip_preamble(
    rendered: String,
    preamble: &str,
    target: TargetBinding,
) -> Result<String, GenerationError> {
    rendered
        .strip_prefix(preamble)
        .map(ToOwned::to_owned)
        .ok_or(GenerationError::EmitterOutput { target })
}

fn render_python_node(ueg: &Ueg) -> Result<String, GenerationError> {
    let NodeKind::Lambda(lambda) = &ueg.nodes[0];
    let first_non_empty = lambda.orig_body.iter().find(|line| !line.trim().is_empty());
    if first_non_empty
        .map(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("def ") || trimmed.starts_with('@')
        })
        .unwrap_or(false)
    {
        let mut output = lambda.orig_body.join("\n");
        output.push('\n');
        return Ok(output);
    }
    Ok(lower_to_python(ueg))
}
