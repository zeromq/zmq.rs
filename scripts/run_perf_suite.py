#!/usr/bin/env python3
"""Run reproducible zmq.rs performance comparisons."""

from __future__ import annotations

import argparse
import itertools
import json
import os
import platform
import re
import shutil
import string
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

DEFAULT_CONFIG = "perf-suite.json"
DEFAULT_OMQ_REV = "a46c1f7"
ZMQRS_IMPLS = {"zmqrs", "libzmq"}


def main() -> int:
    args = parse_args()
    repo_root = Path(args.repo_root).resolve()
    candidate_root = Path(args.candidate_path or repo_root).resolve()
    config = load_config(repo_root / args.config)
    profile = config["profiles"][args.profile]
    impls = parse_csv(args.impl)
    transports = parse_csv(args.transport)
    runtimes = parse_csv(args.runtime) if args.runtime else profile["zmqrs_runtimes"]
    run_id = args.run_id or datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = repo_root / "target" / "perf-runs" / run_id
    if run_dir.exists() and not args.force:
        raise SystemExit(f"run directory already exists: {run_dir} (pass --force to replace)")
    if run_dir.exists():
        shutil.rmtree(run_dir)
    run_dir.mkdir(parents=True)

    manifest = build_manifest(
        args=args,
        config=config,
        repo_root=repo_root,
        candidate_root=candidate_root,
        profile=profile,
        impls=impls,
        transports=transports,
        runtimes=runtimes,
        run_id=run_id,
    )
    write_json(run_dir / "manifest.json", manifest)
    results_path = run_dir / "results.jsonl"

    try:
        if ZMQRS_IMPLS.intersection(impls):
            run_zmqrs_and_libzmq(
                args=args,
                candidate_root=candidate_root,
                run_dir=run_dir,
                results_path=results_path,
                profile=profile,
                impls=impls,
                transports=transports,
                runtimes=runtimes,
                manifest=manifest,
            )
        if "omq" in impls:
            run_omq(
                args=args,
                repo_root=repo_root,
                run_dir=run_dir,
                results_path=results_path,
                config=config,
                profile=profile,
                transports=transports,
                manifest=manifest,
            )
        manifest["status"] = "complete"
        manifest["completed_at"] = now()
        write_json(run_dir / "manifest.json", manifest)
        if not args.no_report and not args.dry_run:
            subprocess.run(
                [sys.executable, str(repo_root / "scripts" / "report_perf_suite.py"), str(run_dir)],
                check=True,
            )
        print(f"perf run: {run_dir}")
        return 0
    except Exception as exc:
        manifest["status"] = "failed"
        manifest["failed_at"] = now()
        manifest["error"] = str(exc)
        write_json(run_dir / "manifest.json", manifest)
        raise


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", default=".")
    parser.add_argument("--candidate-path", help="zmq.rs checkout to benchmark")
    parser.add_argument("--config", default=DEFAULT_CONFIG)
    parser.add_argument("--profile", choices=["smoke", "standard", "full"], default="standard")
    parser.add_argument("--impl", default="zmqrs,libzmq")
    parser.add_argument("--transport", default="tcp,ipc")
    parser.add_argument("--runtime", help="Comma-separated zmq.rs runtime features")
    parser.add_argument("--run-id")
    parser.add_argument("--omq-rev", default=DEFAULT_OMQ_REV)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--toolchain", help="Optional cargo toolchain, for example stable or 1.94.1")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--no-report", action="store_true")
    return parser.parse_args()


def load_config(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != 1:
        raise SystemExit(f"unsupported perf-suite config schema in {path}")
    return data


def parse_csv(value: str) -> list[str]:
    return [item.strip() for item in value.split(",") if item.strip()]


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def build_manifest(
    *,
    args: argparse.Namespace,
    config: dict[str, Any],
    repo_root: Path,
    candidate_root: Path,
    profile: dict[str, Any],
    impls: list[str],
    transports: list[str],
    runtimes: list[str],
    run_id: str,
) -> dict[str, Any]:
    omq_cfg = config["omq"]
    return {
        "schema": 1,
        "status": "running",
        "run_id": run_id,
        "created_at": now(),
        "profile": args.profile,
        "profile_description": profile.get("description"),
        "repo_root": str(repo_root),
        "candidate_path": str(candidate_root),
        "implementations": impls,
        "transports": transports,
        "runtimes": runtimes,
        "omq": {
            "repository": omq_cfg["repository"],
            "revision": args.omq_rev,
            "default_revision": omq_cfg.get("revision"),
        },
        "host": host_metadata(),
        "tools": tool_metadata(cargo_cmd(args)),
        "git": {
            "repo": git_metadata(repo_root),
            "candidate": git_metadata(candidate_root),
        },
        "commands": [],
    }


def host_metadata() -> dict[str, Any]:
    cpu = platform.processor() or platform.machine()
    if sys.platform == "darwin":
        try:
            cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        except (OSError, subprocess.SubprocessError):
            pass
    elif Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8", errors="replace").splitlines():
            if line.lower().startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": cpu,
        "python": sys.version.split()[0],
    }


def tool_metadata(cargo: list[str]) -> dict[str, str | None]:
    return {
        "rustc": command_text(["rustc", "--version"]),
        "cargo": command_text([*cargo, "--version"]),
    }


def git_metadata(root: Path) -> dict[str, str | None]:
    return {
        "sha": command_text(["git", "rev-parse", "HEAD"], cwd=root),
        "branch": command_text(["git", "branch", "--show-current"], cwd=root),
        "dirty": command_text(["git", "status", "--short"], cwd=root),
    }


def command_text(cmd: list[str], cwd: Path | None = None) -> str | None:
    try:
        return subprocess.check_output(cmd, cwd=cwd, text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.SubprocessError):
        return None


def run_zmqrs_and_libzmq(
    *,
    args: argparse.Namespace,
    candidate_root: Path,
    run_dir: Path,
    results_path: Path,
    profile: dict[str, Any],
    impls: list[str],
    transports: list[str],
    runtimes: list[str],
    manifest: dict[str, Any],
) -> None:
    selected_impls = [impl for impl in impls if impl in ZMQRS_IMPLS]
    for bench in profile["benches"]:
        bench_impls = [impl for impl in selected_impls if impl in bench.get("implementations", selected_impls)]
        if not bench_impls:
            continue
        if "libzmq" in bench_impls:
            run_criterion_entry(
                args=args,
                candidate_root=candidate_root,
                run_dir=run_dir,
                results_path=results_path,
                manifest=manifest,
                bench=bench,
                implementation="libzmq",
                runtime="tokio-runtime",
                transports=transports,
                criterion_args=profile["criterion_args"],
            )
        if "zmqrs" in bench_impls:
            for runtime in runtimes:
                run_criterion_entry(
                    args=args,
                    candidate_root=candidate_root,
                    run_dir=run_dir,
                    results_path=results_path,
                    manifest=manifest,
                    bench=bench,
                    implementation="zmqrs",
                    runtime=runtime,
                    transports=transports,
                    criterion_args=profile["criterion_args"],
                )


def run_criterion_entry(
    *,
    args: argparse.Namespace,
    candidate_root: Path,
    run_dir: Path,
    results_path: Path,
    manifest: dict[str, Any],
    bench: dict[str, Any],
    implementation: str,
    runtime: str,
    transports: list[str],
    criterion_args: list[str],
) -> None:
    filters = expand_filters(bench, implementation, transports)
    if not filters:
        return
    filter_regex = "|".join(re.escape(item) for item in filters)
    target_criterion = candidate_root / "target" / "criterion"
    if target_criterion.exists():
        shutil.rmtree(target_criterion)

    cmd = [
        *cargo_cmd(args),
        "bench",
        "--no-default-features",
        "--features",
        ",".join(features_for_runtime(runtime)),
        "--bench",
        bench["name"],
        "--",
        *criterion_args,
        filter_regex,
    ]
    env = os.environ.copy()
    env.update(criterion_env(criterion_args))
    run_command(cmd, cwd=candidate_root, env=env, manifest=manifest, dry_run=args.dry_run)
    artifact_dir = run_dir / "artifacts" / safe_name(f"{implementation}-{runtime}") / bench["name"]
    if not args.dry_run and target_criterion.exists():
        shutil.copytree(target_criterion, artifact_dir / "criterion")
        rows = criterion_rows(
            artifact_dir / "criterion",
            implementation=implementation,
            runtime=runtime.removesuffix("-runtime"),
            suite=bench["name"],
        )
        append_jsonl(results_path, rows)


def features_for_runtime(runtime: str) -> list[str]:
    features = [runtime, "all-transport"]
    if runtime == "async-dispatcher-runtime":
        features.append("async-dispatcher-macros")
    return features


def criterion_env(criterion_args: list[str]) -> dict[str, str]:
    env: dict[str, str] = {}
    pairs = list(zip(criterion_args, criterion_args[1:]))
    for flag, value in pairs:
        if flag == "--sample-size":
            env["ZMQRS_BENCH_SAMPLE_SIZE"] = value
        elif flag == "--measurement-time":
            env["ZMQRS_BENCH_MEASUREMENT_MS"] = str(int(float(value) * 1000))
        elif flag == "--warm-up-time":
            env["ZMQRS_BENCH_WARMUP_MS"] = str(int(float(value) * 1000))
    return env


def expand_filters(bench: dict[str, Any], implementation: str, transports: list[str]) -> list[str]:
    values: dict[str, list[Any]] = {
        "impl": [implementation],
        "transport": transports,
        "size": bench.get("sizes", [None]),
        "subs": bench.get("subs", [None]),
        "frames": bench.get("frames", [None]),
    }
    filters: list[str] = []
    for template in bench["templates"]:
        fields = [field for _, field, _, _ in string.Formatter().parse(template) if field]
        products = [[value for value in values[field] if value is not None] for field in fields]
        for combo in itertools.product(*products):
            filters.append(template.format(**dict(zip(fields, combo))))
    return sorted(set(filters))


def criterion_rows(
    criterion_dir: Path,
    *,
    implementation: str,
    runtime: str,
    suite: str,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for benchmark_path in criterion_dir.rglob("benchmark.json"):
        if benchmark_path.parent.name != "new":
            continue
        estimates_path = benchmark_path.with_name("estimates.json")
        if not estimates_path.exists():
            continue
        try:
            benchmark = json.loads(benchmark_path.read_text(encoding="utf-8"))
            estimates = json.loads(estimates_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        row = parse_criterion_row(benchmark, estimates, implementation, runtime, suite)
        if row:
            rows.append(row)
    return rows


def parse_criterion_row(
    benchmark: dict[str, Any],
    estimates: dict[str, Any],
    implementation: str,
    runtime: str,
    suite: str,
) -> dict[str, Any] | None:
    group_id = benchmark.get("group_id")
    full_id = benchmark.get("full_id")
    value_str = benchmark.get("value_str")
    if not isinstance(group_id, str) or not isinstance(full_id, str):
        return None
    try:
        size = int(value_str if value_str is not None else full_id.rsplit("/", 1)[-1])
    except (TypeError, ValueError):
        return None
    estimate = estimates.get("slope") or estimates.get("mean")
    if not isinstance(estimate, dict):
        return None
    ns = estimate.get("point_estimate")
    if not isinstance(ns, (int, float)) or ns <= 0:
        return None
    parsed = parse_group(group_id, implementation)
    if parsed is None:
        return None
    throughput = benchmark.get("throughput")
    bytes_per_iter = None
    if isinstance(throughput, dict) and isinstance(throughput.get("Bytes"), int):
        bytes_per_iter = throughput["Bytes"]
    throughput_bps = (bytes_per_iter * 1_000_000_000.0 / ns) if bytes_per_iter else None
    return {
        "source": "criterion",
        "suite": suite,
        "implementation": implementation,
        "runtime": runtime,
        "transport": parsed["transport"],
        "workload": parsed["workload"],
        "variant": parsed.get("variant"),
        "peers": parsed.get("peers", 1),
        "message_size": size,
        "latency_ns_per_iter": ns,
        "throughput_bytes_per_second": throughput_bps,
        "full_id": full_id,
    }


def parse_group(group_id: str, implementation: str) -> dict[str, Any] | None:
    parts = group_id.split("/")
    if parts[:1] == ["codec"]:
        return {"workload": f"codec_{parts[1]}", "transport": "memory", "peers": 1}
    if not parts or parts[0] != implementation:
        return None
    if len(parts) >= 5 and parts[1] == "throughput":
        workload, transport, variant = parts[2], parts[3], parts[4]
    elif len(parts) >= 4:
        workload, transport, variant = parts[1], parts[2], parts[3]
    elif len(parts) >= 3:
        workload, transport, variant = parts[1], parts[2], None
    else:
        return None
    peers = 1
    if variant:
        match = re.search(r"(subs|peers?)=(\d+)", variant)
        if match:
            peers = int(match.group(2))
    return {"workload": workload, "transport": transport, "variant": variant, "peers": peers}


def run_omq(
    *,
    args: argparse.Namespace,
    repo_root: Path,
    run_dir: Path,
    results_path: Path,
    config: dict[str, Any],
    profile: dict[str, Any],
    transports: list[str],
    manifest: dict[str, Any],
) -> None:
    omq_dir = repo_root / "target" / "perf-deps" / "omq.rs"
    ensure_omq_clone(omq_dir=omq_dir, repo=config["omq"]["repository"], rev=args.omq_rev, manifest=manifest, dry_run=args.dry_run)
    omq_profile = profile["omq"]
    omq_transports = list(transports)
    if omq_profile.get("include_inproc") and "inproc" not in omq_transports:
        omq_transports.append("inproc")
    for package in omq_profile["packages"]:
        for bench_name in omq_profile["benches"]:
            suffix = safe_name(f"{manifest['run_id']}-{package}-{bench_name}")
            cmd = [
                *cargo_cmd(args),
                "bench",
                "--manifest-path",
                str(omq_dir / "Cargo.toml"),
                "-p",
                package,
                "--bench",
                bench_name,
            ]
            env = os.environ.copy()
            env.update(
                {
                    "OMQ_BENCH_RUN_ID": manifest["run_id"],
                    "OMQ_BENCH_RESULTS_SUFFIX": suffix,
                    "OMQ_BENCH_TRANSPORTS": ",".join(omq_transports),
                    "OMQ_BENCH_SIZES": ",".join(str(s) for s in omq_profile["sizes"]),
                    "OMQ_BENCH_PEERS": ",".join(str(p) for p in omq_profile["peers"]),
                    "OMQ_BENCH_ROUND_MS": str(omq_profile["round_ms"]),
                    "OMQ_BENCH_ROUNDS": str(omq_profile["rounds"]),
                }
            )
            run_command(cmd, cwd=Path(tempfile.gettempdir()), env=env, manifest=manifest, dry_run=args.dry_run)
            if args.dry_run:
                continue
            source_path = omq_dir / package / "benches" / f"results_{suffix}.jsonl"
            artifact_dir = run_dir / "artifacts" / package / bench_name
            artifact_dir.mkdir(parents=True, exist_ok=True)
            if source_path.exists():
                shutil.copy2(source_path, artifact_dir / source_path.name)
                append_jsonl(results_path, omq_rows(source_path, package))


def ensure_omq_clone(*, omq_dir: Path, repo: str, rev: str, manifest: dict[str, Any], dry_run: bool) -> None:
    if not omq_dir.exists():
        omq_dir.parent.mkdir(parents=True, exist_ok=True)
        run_command(["git", "clone", repo, str(omq_dir)], cwd=omq_dir.parent, manifest=manifest, dry_run=dry_run)
    run_command(["git", "fetch", "--tags", "origin"], cwd=omq_dir, manifest=manifest, dry_run=dry_run)
    run_command(["git", "checkout", rev], cwd=omq_dir, manifest=manifest, dry_run=dry_run)


def omq_rows(path: Path, package: str) -> list[dict[str, Any]]:
    rows = []
    runtime = package.removeprefix("omq-")
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        data = json.loads(line)
        rows.append(
            {
                "source": "omq",
                "suite": data["pattern"],
                "implementation": "omq",
                "runtime": runtime,
                "transport": data["transport"],
                "workload": data["pattern"],
                "variant": f"{data['peers']}peer",
                "peers": data["peers"],
                "message_size": data["msg_size"],
                "latency_ns_per_iter": None,
                "throughput_bytes_per_second": data["mbps"] * 1_000_000.0,
                "full_id": f"omq/{runtime}/{data['pattern']}/{data['transport']}/{data['peers']}peer/{data['msg_size']}",
            }
        )
    return rows


def run_command(
    cmd: list[str],
    *,
    cwd: Path,
    manifest: dict[str, Any],
    dry_run: bool,
    env: dict[str, str] | None = None,
) -> None:
    manifest["commands"].append({"cwd": str(cwd), "cmd": cmd, "dry_run": dry_run})
    print("+", " ".join(cmd))
    if not dry_run:
        subprocess.run(cmd, cwd=cwd, env=env, check=True)


def cargo_cmd(args: argparse.Namespace) -> list[str]:
    cmd = [args.cargo]
    if args.toolchain:
        cmd.append(f"+{args.toolchain}")
    return cmd


def append_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    if not rows:
        return
    with path.open("a", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")


def write_json(path: Path, data: dict[str, Any]) -> None:
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-")


if __name__ == "__main__":
    raise SystemExit(main())
