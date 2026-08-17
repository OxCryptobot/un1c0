#!/usr/bin/env python3
"""Generate a self-contained interactive benchmark dashboard."""
from __future__ import annotations

import json
import math
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
            name="repo baseline p95",
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
            name="repo optimized p95",
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
            name="repo baseline throughput",
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
            name="repo optimized throughput",
            legendgroup="repository-profile",
            showlegend=False,
            line={"color": "#0f766e", "width": 2},
            mode="lines+markers",
            hovertemplate="concurrency=%{x}<br>optimized=%{y:,.2f} ops/s<extra></extra>",
        ),
        row=2,
        col=2,
    )

def log_ticks(values: list[float]) -> tuple[list[float], list[str]]:
    positive = [value for value in values if value > 0]
    if not positive:
        return [], []
    lower = 10 ** math.floor(math.log10(min(positive)))
    upper = 10 ** math.ceil(math.log10(max(positive)))
    ticks: list[float] = []
    labels: list[str] = []
    exponent = math.floor(math.log10(lower))
    while 10**exponent <= upper:
        for mantissa in (1, 2, 5):
            value = mantissa * (10**exponent)
            if lower <= value <= upper:
                ticks.append(value)
                if value >= 1_000_000:
                    labels.append(f"{value / 1_000_000:g}M")
                elif value >= 1_000:
                    labels.append(f"{value / 1_000:g}K")
                elif value >= 1:
                    labels.append(f"{value:g}")
                else:
                    labels.append(f"{value:.3g}")
        exponent += 1
    return ticks, labels

latency_values = [row["p95_ns"] / 1_000_000 for row in rows]
throughput_values = [row["throughput_ops_per_sec"] for row in rows]
if profile:
    latency_values.extend(
        value
        for row in profile
        for value in (row["baseline_p95_ms"], row["optimized_p95_ms"])
    )
    throughput_values.extend(
        value
        for row in profile
        for value in (
            row["baseline_throughput_ops_per_sec"],
            row["optimized_throughput_ops_per_sec"],
        )
    )
latency_ticks, latency_labels = log_ticks(latency_values)
throughput_ticks, throughput_labels = log_ticks(throughput_values)

fig.update_xaxes(title_text="Worker concurrency", tickmode="array", tickvals=concurrencies)
fig.update_yaxes(
    title_text="p95 latency (ms; log scale)",
    type="log",
    tickmode="array",
    tickvals=latency_ticks,
    ticktext=latency_labels,
    row=1,
    col=1,
)
fig.update_yaxes(
    title_text="throughput (ops/s; log scale)",
    type="log",
    tickmode="array",
    tickvals=throughput_ticks,
    ticktext=throughput_labels,
    row=1,
    col=2,
)
fig.update_yaxes(
    title_text="p95 latency (ms; log scale)",
    type="log",
    tickmode="array",
    tickvals=latency_ticks,
    ticktext=latency_labels,
    row=2,
    col=1,
)
fig.update_yaxes(
    title_text="throughput (ops/s; log scale)",
    type="log",
    tickmode="array",
    tickvals=throughput_ticks,
    ticktext=throughput_labels,
    row=2,
    col=2,
)
fig.update_layout(
    title="un1c0 interactive performance dashboard",
    template="plotly_white",
    height=1050,
    width=1500,
    hovermode="x unified",
    legend={"orientation": "v", "x": 1.02, "y": 1.0, "title": {"text": "Benchmark series"}},
    margin={"l": 80, "r": 260, "t": 90, "b": 70},
)
fig.write_html(ROOT / "benchmark_dashboard.html", include_plotlyjs=True, full_html=True)
print(json.dumps({"output": "benchmark_dashboard.html", "operations": operations, "concurrencies": concurrencies}, indent=2))
