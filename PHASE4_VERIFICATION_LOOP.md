# Phase 4 — Safe Verification Loop

## Objective

A coding agent must not treat generated source as success. Phase 4 turns every edit into a bounded verification workflow that executes compilers, linters, formatters, tests, and security checks in an isolated workspace and records evidence sufficient to reproduce the result.

The verifier is a **policy-controlled tool**, not a convenience wrapper around the host shell.

## Verification graph

```text
Run checkpoint
    |
    v
Snapshot + changed-file manifest
    |
    v
Patch preconditions / path policy
    |
    +--> formatter and linter
    |
    +--> parser / typecheck / compiler
    |
    +--> unit tests
    |
    +--> integration and golden tests
    |
    +--> security and dependency checks
    |
    v
Aggregate evidence
    |
    +--> Passed: accept candidate
    +--> Failed: create bounded repair task
    +--> Timeout/Unavailable/PolicyBlocked: fail closed
```

Independent gates may run in parallel only when they use isolated output directories and the scheduler has a bounded concurrency budget. Compilation and tests that mutate shared caches should run sequentially or in isolated cache namespaces.

## Verification manifest

Store the verifier configuration as a typed manifest rather than accepting arbitrary commands from model output.

```json
{
  "id": "rust-default",
  "language": "rust",
  "working_directory": ".",
  "gates": [
    {"id":"format","program":"cargo","args":["fmt","--all","--","--check"],"class":"format"},
    {"id":"check","program":"cargo","args":["check","--all-targets"],"class":"compile"},
    {"id":"test","program":"cargo","args":["test","--all-features","--all-targets"],"class":"test"}
  ],
  "budgets": {
    "wall_clock_ms": 600000,
    "output_bytes": 1048576,
    "processes": 128,
    "memory_bytes": 4294967296,
    "network": "disabled"
  }
}
```

The manifest must be checked into the repository or supplied by a trusted user/configuration layer. The model may request a named verifier profile but may not construct an arbitrary `program` or replace its arguments.

## Execution boundary

### Local development mode

Use a dedicated workspace root, canonicalize paths, reject traversal and symlink escapes, set environment variables explicitly, disable secrets by default, and capture all output. Local execution is suitable for trusted repositories and developer iteration but is not a security boundary against a hostile build script.

### Sandboxed mode

For untrusted repositories or automatic execution, run the verifier in a disposable rootless container or equivalent sandbox with:

- no host filesystem mounts except the workspace and explicitly approved read-only caches;
- non-root user, dropped capabilities, read-only base image, and writable temporary workspace only;
- network disabled by default, with explicit per-gate egress exceptions;
- PID, CPU, memory, disk, file-count, and output budgets;
- default seccomp profile or a stricter allowlist, plus AppArmor/SELinux where available;
- process-tree cleanup on timeout and cancellation;
- no host Docker socket, SSH agent, cloud credentials, browser cookies, or secret environment variables;
- immutable toolchain/image digest recorded in the evidence.

Docker documents seccomp as a syscall allowlist that denies by default and recommends retaining the default profile rather than weakening it.[3] OpenHands documents a complementary pattern in which actions are analyzed for risk and confirmation policy decides whether risky actions can proceed.[2] UN1C⓪ should combine both: static capability policy first, optional risk analysis second, and explicit approval for high-risk gates.

## Evidence model

Every gate emits a `VerificationResult`:

```rust
pub enum VerificationStatus {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Unavailable,
    PolicyBlocked,
}

pub struct VerificationResult {
    pub run_id: String,
    pub gate_id: String,
    pub status: VerificationStatus,
    pub program: String,
    pub args_hash: String,
    pub workspace_tree_before: String,
    pub workspace_tree_after: String,
    pub toolchain_digest: Option<String>,
    pub started_at_ms: u128,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<Diagnostic>,
}
```

Bound stdout and stderr by bytes, preserve truncation markers, redact likely secrets, and hash the unredacted stream only if privacy policy permits. Capture exit code, signal, timeout, cancellation, and whether the process tree was fully reaped. Never convert `Unavailable`, `TimedOut`, or `PolicyBlocked` into `Passed`.

## Failure and repair loop

1. Run the smallest gate that can detect the changed failure. If the patch does not parse, do not spend time running the full test suite.
2. Classify diagnostics into syntax, type, compile, test assertion, environment, timeout, dependency, policy, or unknown.
3. Produce a bounded repair task containing gate ID, diagnostic spans, changed-file manifest, last accepted checkpoint, and remaining budget.
4. Permit only a limited number of repair iterations per run. Require each new patch to reference the diagnostic and pass patch preconditions.
5. Re-run the failing gate, then all required gates. A previously passing gate that regresses fails the candidate.
6. Roll back to the last accepted checkpoint on iteration exhaustion, unrelated-file changes, output-budget violation, policy violation, or verification regression.
7. Report evidence and unresolved failures to the user; do not claim success based on the model’s explanation.

## Safety and approval policy

| Operation | Default | Approval |
|---|---|---|
| Parse source / inspect diff | Allowed | None |
| Formatter / linter with no network | Allowed in trusted workspace | Policy may require approval in hostile repos |
| Compiler / unit tests | Allowed only through named verifier profile | Required for untrusted repositories |
| Network-enabled integration test | Denied | Explicit gate and user approval |
| Package installation | Denied | Explicit approval, pinned source, isolated cache |
| Database/cloud/browser integration | Denied | Explicit connector policy and approval |
| Destructive cleanup or migration | Denied | Explicit approval and rollback plan |

Do not use an LLM as the sole security decision-maker. A separate analyzer may add risk annotations, but deterministic policy, sandbox boundaries, and user approval remain authoritative.[2]

## Acceptance tests

1. A valid manifest runs and records all gate evidence.
2. An unknown program, argument mutation, path escape, symlink escape, secret environment variable, or network request is blocked.
3. A compiler timeout kills the entire process tree and returns `TimedOut`.
4. Truncated output is marked and never treated as a clean pass.
5. A failing unit test creates a repair task with the failing test and diagnostic span.
6. A repair that fixes one gate but breaks a previously passing gate is rejected.
7. A missing compiler or unavailable container runtime returns `Unavailable`, not `Passed`.
8. Replaying a manifest against the same workspace snapshot produces the same args hash and comparable evidence.
9. Cancellation is idempotent, journaled, and leaves no verifier child process behind.
10. All events redact secrets and include enough metadata to reproduce the gate selection.

## References

[1]: https://docs.openai.com "Provider structured-output reference is maintained separately in PHASE2_PROVIDER_ROUTING.md"
[2]: https://docs.openhands.dev/sdk/guides/security "OpenHands Security & Action Confirmation"
[3]: https://docs.docker.com/engine/security/seccomp/ "Docker Seccomp security profiles"
