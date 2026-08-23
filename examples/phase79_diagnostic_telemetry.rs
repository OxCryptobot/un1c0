use un1c0::emission_diagnostic_instrumentation::{DiagnosticInstrumentation, VerificationOutcome};

fn main() {
    let instrumentation = DiagnosticInstrumentation::enabled(2);
    instrumentation
        .recorder(1, 512)
        .finish(VerificationOutcome::Accepted);
    instrumentation
        .recorder(2, 1024)
        .finish(VerificationOutcome::Rejected);
    let snapshot = instrumentation.snapshot();
    let bytes = snapshot
        .to_versioned_json()
        .expect("versioned telemetry must validate");
    println!(
        "{}",
        String::from_utf8(bytes).expect("telemetry JSON is UTF-8")
    );
}
