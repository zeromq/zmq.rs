#!/usr/bin/env python3
"""Generate Markdown and HTML summaries for run_perf_suite.py output."""

from __future__ import annotations

import argparse
import html
import json
from pathlib import Path
from typing import Any


def main() -> int:
    args = parse_args()
    run_dir = Path(args.run_dir).resolve()
    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    rows = load_rows(run_dir / "results.jsonl")
    summary = build_summary(rows)
    (run_dir / "summary.md").write_text(render_markdown(manifest, summary), encoding="utf-8")
    (run_dir / "summary.html").write_text(render_html(manifest, summary), encoding="utf-8")
    print(f"wrote {run_dir / 'summary.md'}")
    print(f"wrote {run_dir / 'summary.html'}")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir")
    return parser.parse_args()


def load_rows(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def build_summary(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    baseline = {
        row_key(row): row
        for row in rows
        if row.get("implementation") == "zmqrs" and row.get("runtime") == "tokio"
    }
    summary = []
    for row in rows:
        item = dict(row)
        base = baseline.get(row_key(row))
        item["ratio_vs_zmqrs_tokio"] = ratio_against(row, base)
        summary.append(item)
    return sorted(summary, key=sort_key)


def ratio_against(row: dict[str, Any], base: dict[str, Any] | None) -> float | None:
    if base is None:
        return None
    value = row.get("throughput_bytes_per_second")
    base_value = base.get("throughput_bytes_per_second")
    if isinstance(value, (int, float)) and isinstance(base_value, (int, float)) and base_value > 0:
        return value / base_value
    latency = row.get("latency_ns_per_iter")
    base_latency = base.get("latency_ns_per_iter")
    if isinstance(latency, (int, float)) and isinstance(base_latency, (int, float)) and latency > 0:
        return base_latency / latency
    return None


def row_key(row: dict[str, Any]) -> tuple[Any, ...]:
    return (
        row.get("workload"),
        row.get("transport"),
        row.get("peers"),
        row.get("message_size"),
    )


def sort_key(row: dict[str, Any]) -> tuple[Any, ...]:
    return (
        str(row.get("workload")),
        str(row.get("transport")),
        int(row.get("peers") or 0),
        int(row.get("message_size") or 0),
        str(row.get("implementation")),
        str(row.get("runtime")),
    )


def render_markdown(manifest: dict[str, Any], rows: list[dict[str, Any]]) -> str:
    lines = [
        f"# Performance Run {manifest.get('run_id')}",
        "",
        f"- status: `{manifest.get('status')}`",
        f"- profile: `{manifest.get('profile')}`",
        f"- candidate: `{manifest.get('candidate_path')}`",
        f"- candidate git: `{(manifest.get('git') or {}).get('candidate', {}).get('sha')}`",
        f"- transports: `{', '.join(manifest.get('transports', []))}`",
        f"- implementations: `{', '.join(manifest.get('implementations', []))}`",
        f"- OMQ revision: `{(manifest.get('omq') or {}).get('revision')}`",
        "",
    ]
    if not rows:
        lines.extend(["No benchmark rows were captured.", ""])
        return "\n".join(lines)
    lines.extend(
        [
            "| workload | transport | peers | size | implementation | runtime | throughput | latency | ratio vs zmq.rs tokio |",
            "| --- | --- | ---: | ---: | --- | --- | ---: | ---: | ---: |",
        ]
    )
    for row in rows:
        lines.append(
            "| {workload} | {transport} | {peers} | {size} | {implementation} | {runtime} | {throughput} | {latency} | {ratio} |".format(
                workload=row.get("workload", ""),
                transport=row.get("transport", ""),
                peers=row.get("peers", ""),
                size=row.get("message_size", ""),
                implementation=row.get("implementation", ""),
                runtime=row.get("runtime", ""),
                throughput=format_bps(row.get("throughput_bytes_per_second")),
                latency=format_ns(row.get("latency_ns_per_iter")),
                ratio=format_ratio(row.get("ratio_vs_zmqrs_tokio")),
            )
        )
    lines.append("")
    return "\n".join(lines)


def render_html(manifest: dict[str, Any], rows: list[dict[str, Any]]) -> str:
    body = markdown_to_html(render_markdown(manifest, rows))
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Performance Run {html.escape(str(manifest.get('run_id')))}</title>
<style>
body {{ font: 14px/1.45 -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif; margin: 32px; color: #17202a; }}
code {{ background: #f3f4f6; padding: 1px 4px; border-radius: 4px; }}
table {{ border-collapse: collapse; width: 100%; margin-top: 18px; }}
th, td {{ border-bottom: 1px solid #e5e7eb; padding: 6px 8px; text-align: left; }}
th {{ background: #f8fafc; position: sticky; top: 0; }}
td:nth-child(3), td:nth-child(4), td:nth-child(7), td:nth-child(8), td:nth-child(9) {{ text-align: right; font-variant-numeric: tabular-nums; }}
</style>
</head>
<body>
{body}
</body>
</html>
"""


def markdown_to_html(md: str) -> str:
    lines = md.splitlines()
    out: list[str] = []
    in_list = False
    in_table = False
    for line in lines:
        if line.startswith("# "):
            close_blocks(out, in_list, in_table)
            in_list = in_table = False
            out.append(f"<h1>{html.escape(line[2:])}</h1>")
        elif line.startswith("- "):
            if not in_list:
                out.append("<ul>")
                in_list = True
            out.append(f"<li>{inline_code(line[2:])}</li>")
        elif line.startswith("| ") and not line.startswith("| ---"):
            if in_list:
                out.append("</ul>")
                in_list = False
            if not in_table:
                out.append("<table>")
                in_table = True
            cells = [cell.strip() for cell in line.strip("|").split("|")]
            tag = "th" if cells and cells[0] == "workload" else "td"
            out.append("<tr>" + "".join(f"<{tag}>{html.escape(cell)}</{tag}>" for cell in cells) + "</tr>")
        elif line.startswith("| ---"):
            continue
        elif line.strip():
            close_blocks(out, in_list, in_table)
            in_list = in_table = False
            out.append(f"<p>{inline_code(line)}</p>")
    close_blocks(out, in_list, in_table)
    return "\n".join(out)


def close_blocks(out: list[str], in_list: bool, in_table: bool) -> None:
    if in_list:
        out.append("</ul>")
    if in_table:
        out.append("</table>")


def inline_code(text: str) -> str:
    parts = text.split("`")
    rendered = []
    for idx, part in enumerate(parts):
        escaped = html.escape(part)
        rendered.append(f"<code>{escaped}</code>" if idx % 2 else escaped)
    return "".join(rendered)


def format_bps(value: Any) -> str:
    if not isinstance(value, (int, float)):
        return ""
    mb = value / 1_000_000.0
    if mb >= 1000:
        return f"{mb / 1000:.2f} GB/s"
    return f"{mb:.1f} MB/s"


def format_ns(value: Any) -> str:
    if not isinstance(value, (int, float)):
        return ""
    if value >= 1_000_000:
        return f"{value / 1_000_000:.3f} ms"
    if value >= 1_000:
        return f"{value / 1_000:.3f} us"
    return f"{value:.1f} ns"


def format_ratio(value: Any) -> str:
    if not isinstance(value, (int, float)):
        return ""
    return f"{value:.2f}x"


if __name__ == "__main__":
    raise SystemExit(main())
