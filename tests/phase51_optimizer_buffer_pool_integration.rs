use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::{generate_incrementally, generate_incrementally_with_pool, TargetBinding};
use un1c0::lock_free_buffer_pool::LockFreeBufferPool;
use un1c0::optimizer::{OptimizerHook, OptimizerPipeline};
use un1c0::walker::{python_to_ueg, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

#[derive(Clone)]
struct CountingHook {
    before: Arc<AtomicUsize>,
    after: Arc<AtomicUsize>,
}

struct RejectingHook;

impl OptimizerHook for RejectingHook {
    fn before_optimize(&self, _ueg: &Ueg) -> Result<(), String> {
        Err("deterministic test rejection".into())
    }

    fn after_optimize(&self, _ueg: &Ueg) -> Result<(), String> {
        Ok(())
    }
}

impl OptimizerHook for CountingHook {
    fn before_optimize(&self, _ueg: &Ueg) -> Result<(), String> {
        self.before.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn after_optimize(&self, _ueg: &Ueg) -> Result<(), String> {
        self.after.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn bounded_pool_reuses_buffers_and_drops_oversize_capacity() {
    let pool = LockFreeBufferPool::new(64, 2).expect("pool");
    {
        let mut buffer = pool.checkout();
        buffer.extend_from_slice(b"short output");
    }
    let first_metrics = pool.metrics();
    assert_eq!(first_metrics.checkouts, 1);
    assert_eq!(first_metrics.returns, 1);

    {
        let reused = pool.checkout();
        assert!(reused.is_empty());
    }
    let mut oversized = pool.checkout();
    oversized.as_mut_vec().reserve(128);
    drop(oversized);
    let metrics = pool.metrics();
    assert!(metrics.reused >= 1);
    assert!(metrics.dropped_oversize >= 1);
}

#[test]
fn bounded_pool_is_safe_under_128_concurrent_checkouts() {
    let pool = Arc::new(LockFreeBufferPool::new(128, 32).expect("pool"));
    let mut handles = Vec::new();
    for index in 0..128usize {
        let pool = Arc::clone(&pool);
        handles.push(thread::spawn(move || {
            let mut buffer = pool.checkout();
            buffer.extend_from_slice(index.to_string().as_bytes());
            assert!(!buffer.is_empty());
        }));
    }
    for handle in handles {
        handle.join().expect("pool worker");
    }
    let metrics = pool.metrics();
    assert_eq!(metrics.checkouts, 128);
    assert_eq!(
        metrics.returns + metrics.dropped_full + metrics.dropped_oversize,
        128
    );
    assert!(metrics.fresh_allocations >= 1);
}

#[test]
fn rooted_optimizer_removes_unreachable_functions_and_runs_hooks() {
    let source = "def entry(value: int) -> int:\n    return helper(value)\n\ndef helper(value: int) -> int:\n    return value\n\ndef dead(value: int) -> int:\n    return value + 2\n\ndef external(value: int) -> int:\n    return unknown(value)\n";
    let ueg = parse_ueg(source);
    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let mut pipeline = OptimizerPipeline::with_roots(["entry"]);
    pipeline.add_hook(CountingHook {
        before: Arc::clone(&before),
        after: Arc::clone(&after),
    });

    let (optimized, stats) = pipeline.optimize(&ueg).expect("optimize");
    assert_eq!(stats.before_nodes, 4);
    assert_eq!(stats.after_nodes, 2);
    assert_eq!(stats.removed_functions, vec!["dead", "external"]);
    assert_eq!(before.load(Ordering::Relaxed), 1);
    assert_eq!(after.load(Ordering::Relaxed), 1);

    for target in TargetBinding::ALL {
        let (output, generation) =
            generate_incrementally(&optimized, target).expect("emit optimized UEG");
        assert_eq!(generation.chunks_emitted, 2);
        assert!(output.contains("entry"));
        assert!(output.contains("helper"));
        assert!(!output.contains("dead"));
        assert!(!output.contains("external"));
    }
}

#[test]
fn pooled_incremental_codegen_reuses_output_buffer() {
    let source = "def first(value: int) -> int:\n    return value\n\ndef second(value: int) -> int:\n    return value + 1\n";
    let ueg = parse_ueg(source);
    let pool = LockFreeBufferPool::new(512, 2).expect("pool");
    let (buffer, stats) = generate_incrementally_with_pool(&ueg, TargetBinding::Rust, &pool)
        .expect("pooled generation");
    let output = String::from_utf8(buffer.as_slice().to_vec()).expect("UTF-8 output");
    assert!(output.contains("fn first("));
    assert!(output.contains("fn second("));
    assert_eq!(stats.chunks_emitted, 2);
    drop(buffer);
    let metrics = pool.metrics();
    assert_eq!(metrics.checkouts, 1);
    assert_eq!(metrics.returns, 1);
}

#[test]
fn optimizer_rejects_hooks_unknown_roots_and_duplicate_functions() {
    let valid = parse_ueg("def one(value: int) -> int:\n    return value\n");
    let mut rejecting = OptimizerPipeline::new();
    rejecting.add_hook(RejectingHook);
    assert!(matches!(
        rejecting.optimize(&valid),
        Err(un1c0::optimizer::OptimizerError::HookRejected {
            phase: "before",
            ..
        })
    ));

    let unknown_root = OptimizerPipeline::with_roots(["missing"]);
    assert!(matches!(
        unknown_root.optimize(&valid),
        Err(un1c0::optimizer::OptimizerError::UnknownRoot { .. })
    ));

    let duplicate = parse_ueg("def one(value: int) -> int:\n    return value\n\ndef one(value: int) -> int:\n    return value + 1\n");
    assert!(matches!(
        OptimizerPipeline::with_roots(["one"]).optimize(&duplicate),
        Err(un1c0::optimizer::OptimizerError::DuplicateFunction { .. })
    ));
}

#[test]
fn optimizer_preserves_all_without_roots_and_rejects_invalid_ueg() {
    let valid = parse_ueg("def one(value: int) -> int:\n    return value\n\ndef two(value: int) -> int:\n    return value + 1\n");
    let (preserved, stats) = OptimizerPipeline::new()
        .optimize(&valid)
        .expect("preserve all");
    assert_eq!(preserved.nodes.len(), 2);
    assert_eq!(stats.removed_nodes, 0);

    let invalid = parse_ueg("def bad(value: int):\n    while value > 0:\n        value -= 1\n");
    let error = OptimizerPipeline::new()
        .optimize(&invalid)
        .expect_err("invalid UEG must fail closed");
    assert!(matches!(
        error,
        un1c0::optimizer::OptimizerError::InvalidUeg { .. }
    ));
}
