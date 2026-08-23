# Phase 58–60 comparison inspection notes

The comparison chart is a 1920×800 PNG with two legible panels: 32-function Rust p50 and p95 latency. The x-axis is the phase number, the y-axis is microseconds, and the legend clearly distinguishes full snapshot capture from the phase-specific incremental path.

The full-capture series is nearly flat across the three artifacts: approximately 647 microseconds p50 and 686–720 microseconds p95. The phase-specific path rises from Phase 58 warm refresh to Phase 59 fingerprint-derived auto-refresh and again to Phase 60 manifest-bound refresh. This is an artifact of changing benchmark scopes and the conservative call-chain/session-construction fixture, not proof of regression. The chart is suitable for stakeholder explanation only when paired with the benchmark-source labels and boundary note.
