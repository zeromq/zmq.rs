#!/usr/bin/env python3
"""Package a perf run directory for sharing without committing generated data."""

from __future__ import annotations

import argparse
import hashlib
import json
import tarfile
from pathlib import Path


DEFAULT_FILES = ("manifest.json", "results.jsonl", "summary.md", "summary.html")


def main() -> int:
    args = parse_args()
    run_dir = Path(args.run_dir).resolve()
    if not run_dir.is_dir():
        raise SystemExit(f"run directory does not exist: {run_dir}")

    manifest_path = run_dir / "manifest.json"
    if not manifest_path.exists():
        raise SystemExit(f"missing manifest.json in {run_dir}")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    run_id = str(manifest.get("run_id") or run_dir.name)

    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    archive = out_dir / f"zmqrs-perf-{safe_name(run_id)}.tar.gz"

    with tarfile.open(archive, "w:gz") as tar:
        for name in DEFAULT_FILES:
            path = run_dir / name
            if path.exists():
                tar.add(path, arcname=f"{run_dir.name}/{name}")
        if args.include_artifacts:
            artifacts = run_dir / "artifacts"
            if artifacts.exists():
                tar.add(artifacts, arcname=f"{run_dir.name}/artifacts")

    digest = sha256_file(archive)
    checksum = archive.with_suffix(archive.suffix + ".sha256")
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    print(archive)
    print(checksum)
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir")
    parser.add_argument("--out-dir", default="target/perf-archives")
    parser.add_argument(
        "--include-artifacts",
        action="store_true",
        help="Include raw Criterion and external benchmark artifacts.",
    )
    return parser.parse_args()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_name(value: str) -> str:
    return "".join(c if c.isalnum() or c in "._-" else "-" for c in value).strip("-")


if __name__ == "__main__":
    raise SystemExit(main())
