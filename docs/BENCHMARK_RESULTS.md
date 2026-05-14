# Benchmark Results

The canonical unit for a performance result is one run directory under
`target/perf-runs/<run-id>/`. These directories are generated artifacts and are
not committed to the repository.

## Sharing Runs

Use the run directory for local analysis, then publish a compressed archive when
the data needs to be shared across machines, pull requests, or issue comments:

```sh
python3 scripts/run_perf_suite.py --profile standard --impl zmqrs,libzmq,omq --transport tcp,ipc --run-id <run-id>
python3 scripts/package_perf_run.py target/perf-runs/<run-id>
```

The package script writes `target/perf-archives/zmqrs-perf-<run-id>.tar.gz` and
a matching `.sha256` file. By default it includes:

- `manifest.json`
- `results.jsonl`
- `summary.md`
- `summary.html`

Use `--include-artifacts` only when raw Criterion directories or external bench
outputs are needed for debugging. Full artifacts are useful, but they are too
large for routine sharing.

## Run IDs

Use stable, searchable run IDs:

```text
<host>-<profile>-<candidate-sha>-<YYYYMMDDTHHMMSSZ>
```

Examples:

```text
macbook-standard-16ae7d1-20260514T190000Z
c7g-4xlarge-standard-16ae7d1-20260514T190000Z
```

The manifest already captures exact git SHAs, tool versions, OS, CPU metadata,
profile, transports, implementations, runtimes, OMQ revision, and commands.
The run ID is for quick human scanning.

## Posting Results

For GitHub discussions or PR comments, post:

- The `summary.md` table.
- The candidate git SHA from `manifest.json`.
- The host CPU/OS from `manifest.json`.
- The archive location and SHA256 checksum.
- Any non-default flags such as `--toolchain`.

For distributed agent runs, store archives outside the repo, for example as
GitHub Actions artifacts or under an S3 prefix such as:

```text
s3://<bucket>/zmq.rs/perf-runs/<run-id>/zmqrs-perf-<run-id>.tar.gz
```

If we later need durable cross-run history in git, commit only a small curated
index such as `benchmarks/results.jsonl` containing one normalized row per
workload/runtime/transport/size. Do not commit complete `target/perf-runs`
directories or raw Criterion artifacts.
