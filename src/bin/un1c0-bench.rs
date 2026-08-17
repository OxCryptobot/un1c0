use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex};
use std::thread;
use std::time::Instant;

use ed25519_dalek::SigningKey;
use un1c0::agentic::{Action, Capability, Plan, Tool, ToolRegistry, ToolSpec, Workspace};
use un1c0::evolution::{CanaryReport, EvaluationCheck, SignedEvolutionProposal, TrustedSignerStore};
use un1c0::provider::{
    FinishReason, ModelProvider, ProviderError, ProviderManifest, ProviderRequest, ProviderResponse,
    ProviderRouter, RouterConfig, RouteOutcome, TaskRisk, Usage,
};
use un1c0::repository::{IndexConfig, RepositoryIndex, SearchOptions};
use un1c0::run_state::{CheckpointStore, RunCheckpoint};
use un1c0::verification::{
    GateClass, NetworkPolicy, VerificationBudget, VerificationGate, VerificationManifest,
    VerifierCatalog,
};

#[derive(Debug, Serialize)]
struct BenchRow {
    operation: String,
    concurrency: usize,
    samples: usize,
    errors: usize,
    elapsed_ms: f64,
    throughput_ops_per_sec: f64,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
}

struct BenchTool {
    spec: ToolSpec,
}

impl Tool for BenchTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, _input: &Value, _workspace: &Workspace) -> Result<Value, un1c0::agentic::AgentError> {
        Ok(json!({"ok": true}))
    }
}

struct MockProvider {
    manifest: ProviderManifest,
}

impl ModelProvider for MockProvider {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    fn complete(&self, _request: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse {
            provider_id: self.manifest.provider_id.clone(),
            model_id: self.manifest.model_id.clone(),
            raw_output: "{}".to_string(),
            structured_output: Some(json!({})),
            refusal: None,
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            latency_ms: 0,
        })
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * percentile + 99) / 100;
    sorted[index.min(sorted.len() - 1)]
}

fn run_load<F>(operation: &str, total: usize, concurrency: usize, operation_fn: F) -> BenchRow
where
    F: Fn(usize) -> bool + Send + Sync + 'static,
{
    let concurrency = concurrency.max(1).min(total.max(1));
    let operation_fn = Arc::new(operation_fn);
    let latencies = Arc::new(Mutex::new(Vec::with_capacity(total)));
    let errors = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();

    thread::scope(|scope| {
        for worker in 0..concurrency {
            let operation_fn = Arc::clone(&operation_fn);
            let latencies = Arc::clone(&latencies);
            let errors = Arc::clone(&errors);
            scope.spawn(move || {
                for sample in (worker..total).step_by(concurrency) {
                    let begin = Instant::now();
                    if !(operation_fn)(sample) {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    let nanos = begin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
                    latencies.lock().expect("latency mutex").push(nanos);
                }
            });
        }
    });

    let elapsed = started.elapsed();
    let mut latencies = latencies.lock().expect("latency mutex").clone();
    latencies.sort_unstable();
    let elapsed_secs = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    BenchRow {
        operation: operation.to_string(),
        concurrency,
        samples: total,
        errors: errors.load(Ordering::Relaxed),
        elapsed_ms: elapsed.as_secs_f64() * 1_000.0,
        throughput_ops_per_sec: total as f64 / elapsed_secs,
        p50_ns: percentile(&latencies, 50),
        p95_ns: percentile(&latencies, 95),
        p99_ns: percentile(&latencies, 99),
    }
}

fn create_fixtures(root: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root.join("src"))?;
    for index in 0..128 {
        let path = root.join("src").join(format!("fixture_{index:03}.rs"));
        let body = format!(
            "pub fn fixture_{index}() -> usize {{ {index} }}\nstruct Agent{index};\n"
        )
        .repeat(8);
        fs::write(path, body)?;
    }
    Ok(())
}

fn benchmark_plan() -> (Arc<Plan>, Arc<ToolRegistry>) {
    let spec = ToolSpec {
        name: "bench.read".to_string(),
        description: "benchmark read-only tool".to_string(),
        capabilities: BTreeSet::from([Capability::WorkspaceRead]),
        input_schema: json!({"type": "object"}),
        default_timeout_ms: 1_000,
    };
    let mut registry = ToolRegistry::new();
    registry.register(BenchTool { spec }).expect("register benchmark tool");
    let action = Action {
        id: "inspect".to_string(),
        tool: "bench.read".to_string(),
        input: json!({"query": "fixture"}),
        depends_on: Vec::new(),
        capabilities: vec![Capability::WorkspaceRead],
        timeout_ms: Some(1_000),
    };
    let plan = Plan {
        id: "bench-plan".to_string(),
        goal: "benchmark local agent contracts".to_string(),
        actions: vec![action],
        max_steps: 4,
        max_output_bytes: 64 * 1024,
    };
    (Arc::new(plan), Arc::new(registry))
}

fn benchmark_provider() -> Arc<ProviderRouter> {
    let manifest = ProviderManifest {
        provider_id: "mock.local".to_string(),
        model_id: "mock-plan-v1".to_string(),
        schema_versions: BTreeSet::from(["plan.v1".to_string()]),
        structured_output: true,
        max_context_tokens: 8_192,
        max_output_tokens: 1_024,
        capabilities: BTreeSet::new(),
        quality_score: 100,
        cost_per_million_tokens: 0,
        latency_ms: 0,
        healthy: true,
    };
    Arc::new(ProviderRouter::new(
        vec![Arc::new(MockProvider { manifest })],
        RouterConfig {
            max_retries_per_provider: 0,
            max_attempts_total: 1,
            ..RouterConfig::default()
        },
    ))
}

fn provider_request() -> ProviderRequest {
    ProviderRequest {
        request_id: "bench-request".to_string(),
        goal: "validate a deterministic benchmark plan".to_string(),
        context: Vec::new(),
        schema_version: "plan.v1".to_string(),
        context_tokens: 1,
        max_output_tokens: 128,
        deadline_ms: 1_000,
        required_capabilities: BTreeSet::new(),
        risk: TaskRisk::Low,
        minimum_quality_score: 0,
    }
}

fn verification_manifest() -> VerificationManifest {
    VerificationManifest {
        id: "bench-verification".to_string(),
        language: "rust".to_string(),
        gates: vec![VerificationGate {
            id: "compile".to_string(),
            program: "cargo".to_string(),
            args: vec!["--version".to_string()],
            class: GateClass::Compile,
            working_directory: None,
            required_capabilities: BTreeSet::from([Capability::ProcessExec]),
            timeout_ms: Some(1_000),
            network: false,
        }],
        budget: VerificationBudget {
            network: NetworkPolicy::Disabled,
            ..VerificationBudget::default()
        },
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("un1c0-bench-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    create_fixtures(&root)?;
    let workspace = Arc::new(Workspace::new(&root)?);
    let (plan, registry) = benchmark_plan();
    let index = Arc::new(RepositoryIndex::build(&root, &IndexConfig::default())?);
    let provider = benchmark_provider();
    let request = Arc::new(provider_request());
    let catalog = Arc::new(VerifierCatalog::safe_local());
    let manifest = Arc::new(verification_manifest());

    let files = BTreeMap::from([(String::from("src/fixture_000.rs"), String::from("content"))]);
    let proposal = un1c0::agentic::EvolutionProposal::new(
        "benchmark proposal",
        files,
        "cargo test",
        "low",
    )?;
    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let signed = SignedEvolutionProposal::sign(proposal, "bench:operator", &signing_key)?;
    let mut trusted = TrustedSignerStore::default();
    trusted.trust_public_key(&signed.signer_id, &signed.public_key)?;
    let signed = Arc::new(signed);
    let trusted = Arc::new(trusted);
    let changed_paths = Arc::new(vec![String::from("src/fixture_000.rs")]);
    let check = Arc::new(EvaluationCheck::from_output(
        "bench-check",
        true,
        Some(0),
        "ok",
        "",
        1,
    )?);

    let concurrencies = [1usize, 2, 4, 8];
    let samples = 2_000usize;
    let mut rows = Vec::new();
    for &concurrency in &concurrencies {
        let plan_for_validate = Arc::clone(&plan);
        let registry_for_validate = Arc::clone(&registry);
        rows.push(run_load("plan_validate", samples, concurrency, move |_| {
            plan_for_validate.validate(&registry_for_validate).is_ok()
        }));

        let index = Arc::clone(&index);
        rows.push(run_load("repository_search", samples, concurrency, move |_| {
            index
                .search("fixture Agent", &SearchOptions::default())
                .map(|matches| !matches.is_empty())
                .unwrap_or(false)
        }));

        let provider = Arc::clone(&provider);
        let request = Arc::clone(&request);
        rows.push(run_load("provider_route", samples, concurrency, move |_| {
            matches!(provider.complete(&request), Ok(RouteOutcome { .. }))
        }));

        let plan_for_checkpoint = Arc::clone(&plan);
        let root = root.clone();
        rows.push(run_load("checkpoint_save_load", samples, concurrency, move |sample| {
            let path = root.join("checkpoints").join(format!("{sample}.json"));
            let store = CheckpointStore::new(&path);
            let checkpoint = RunCheckpoint::new(plan_for_checkpoint.as_ref(), format!("run-{sample}"), Vec::new());
            store.save(&checkpoint).is_ok()
                && store.load().ok().flatten().is_some_and(|loaded| loaded.validate_for(plan_for_checkpoint.as_ref()).is_ok())
                && store.clear().is_ok()
        }));

        let workspace_for_verification = Arc::clone(&workspace);
        let catalog_for_verification = Arc::clone(&catalog);
        let manifest_for_verification = Arc::clone(&manifest);
        rows.push(run_load("verification_manifest", samples, concurrency, move |_| {
            catalog_for_verification
                .validate_manifest(&manifest_for_verification, &workspace_for_verification)
                .is_ok()
        }));

        let signed = Arc::clone(&signed);
        let trusted = Arc::clone(&trusted);
        rows.push(run_load("evolution_signature_verify", samples, concurrency, move |_| {
            signed.verify_with_trust(&trusted).is_ok()
        }));

        let workspace_for_canary = Arc::clone(&workspace);
        let changed_paths = Arc::clone(&changed_paths);
        let check = Arc::clone(&check);
        rows.push(run_load("canary_report_from_workspace", samples, concurrency, move |_| {
            CanaryReport::from_workspace(
                workspace_for_canary.root(),
                "bench-run",
                vec![(*check).clone()],
                changed_paths.as_slice(),
            )
            .and_then(|report| report.evidence_digest())
            .is_ok()
        }));
    }

    println!("{}", serde_json::to_string_pretty(&rows)?);
    fs::remove_dir_all(root)?;
    Ok(())
}
