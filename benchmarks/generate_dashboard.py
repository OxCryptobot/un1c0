#!/usr/bin/env python3
"""Generate a self-contained interactive benchmark dashboard."""
from __future__ import annotations

import json
from pathlib import Path

import plotly.graph_objects as go
from plotly.subplots import make_subplots

ROOT = Path(__file__).resolve().parent
rows = json.loads((ROOT / "agent_benchmark.json").read_text())
profile_path = ROOT / "repository_search_profile.json"
profile = json.loads(profile_path.read_text()) if profile_path.exists() else []
operations = sorted({row["operation"] for row in rows})
concurrencies = sorted({row["concurrency"] for row in rows})
colors = {
    operation: color
    for operation, color in zip(
        operations,
        ["#2563eb", "#ea580c", "#16a34a", "#dc2626", "#9333ea", "#92400e", "#db2777"],
    )
}

fig = make_subplots(
    rows=2,
    cols=2,
    subplot_titles=(
        "p95 latency by concurrency",
        "Throughput by concurrency",
        "Repository search p95 before vs. after cache",
        "Repository search throughput before vs. after cache",
    ),
    vertical_spacing=0.16,
    horizontal_spacing=0.10,
)

for operation in operations:
    series = sorted(
        [row for row in rows if row["operation"] == operation],
        key=lambda row: row["concurrency"],
    )
    customdata = [[row["samples"], row["errors"], row["p99_ns"] / 1_000_000] for row in series]
    fig.add_trace(
        go.Scatter(
            x=[row["concurrency"] for row in series],
            y=[row["p95_ns"] / 1_000_000 for row in series],
            name=operation,
            legendgroup=operation,
            line={"color": colors[operation], "width": 2},
            mode="lines+markers",
            customdata=customdata,
            hovertemplate=(
                "operation=%{fullData.name}<br>concurrency=%{x}<br>"
                "p95=%{y:.6f} ms<br>samples=%{customdata[0]}<br>"
                "errors=%{customdata[1]}<br>p99=%{customdata[2]:.6f} ms<extra></extra>"
            ),
        ),
        row=1,
        col=1,
    )
    fig.add_trace(
        go.Scatter(
            x=[row["concurrency"] for row in series],
            y=[row["throughput_ops_per_sec"] for row in series],
            name=operation,
            legendgroup=operation,
            showlegend=False,
            line={"color": colors[operation], "width": 2},
            mode="lines+markers",
            hovertemplate=(
                "operation=%{fullData.name}<br>concurrency=%{x}<br>"
                "throughput=%{y:,.2f} ops/s<extra></extra>"
            ),
        ),
        row=1,
        col=2,
    )

if profile:
    x = [row["concurrency"] for row in profile]
    fig.add_trace(
        go.Scatter(
            x=x,
            y=[row["baseline_p95_ms"] for row in profile],
            name="baseline p95",
            legendgroup="repository-profile",
            line={"color": "#64748b", "dash": "dash", "width": 2},
            mode="lines+markers",
            hovertemplate="concurrency=%{x}<br>baseline p95=%{y:.3f} ms<extra></extra>",
        ),
        row=2,
        col=1,
    )
    fig.add_trace(
        go.Scatter(
            x=x,
            y=[row["optimized_p95_ms"] for row in profile],
            name="optimized p95",
            legendgroup="repository-profile",
            line={"color": "#0f766e", "width": 2},
            mode="lines+markers",
            hovertemplate="concurrency=%{x}<br>optimized p95=%{y:.3f} ms<extra></extra>",
        ),
        row=2,
        col=1,
    )
    fig.add_trace(
        go.Scatter(
            x=x,
            y=[row["baseline_throughput_ops_per_sec"] for row in profile],
            name="baseline throughput",
            legendgroup="repository-profile",
            showlegend=False,
            line={"color": "#64748b", "dash": "dash", "width": 2},
            mode="lines+markers",
            hovertemplate="concurrency=%{x}<br>baseline=%{y:,.2f} ops/s<extra></extra>",
        ),
        row=2,
        col=2,
    )
    fig.add_trace(
        go.Scatter(
            x=x,
            y=[row["optimized_throughput_ops_per_sec"] for row in profile],
            name="optimized throughput",
            legendgroup="repository-profile",
            showlegend=False,
            line={"color": "#0f766e", "width": 2},
            mode="lines+markers",
            hovertemplate="concurrency=%{x}<br>optimized=%{y:,.2f} ops/s<extra></extra>",
        ),
        row=2,
        col=2,
    )

fig.update_xaxes(title_text="Worker concurrency", tickmode="array", tickvals=concurrencies)
fig.update_yaxes(title_text="p95 latency (ms)", type="log", row=1, col=1)
fig.update_yaxes(title_text="throughput (ops/s)", type="log", row=1, col=2)
fig.update_yaxes(title_text="p95 latency (ms)", type="log", row=2, col=1)
fig.update_yaxes(title_text="throughput (ops/s)", type="log", row=2, col=2)
fig.update_layout(
    title="un1c0 interactive performance dashboard",
    template="plotly_white",
    height=1050,
    width=1500,
    hovermode="x unified",
    legend={"orientation": "v", "x": 1.02, "y": 1.0},
    margin={"l": 80, "r": 260, "t": 90, "b": 70},
)
fig.write_html(ROOT / "benchmark_dashboard.html", include_plotlyjs=True, full_html=True)
print(json.dumps({"output": "benchmark_dashboard.html", "operations": operations, "concurrencies": concurrencies}, indent=2))
