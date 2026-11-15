#!/usr/bin/env python3
"""Unit tests for run_perf_suite.py configuration expansion."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "scripts" / "run_perf_suite.py"

spec = importlib.util.spec_from_file_location("run_perf_suite", MODULE_PATH)
assert spec is not None
run_perf_suite = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(run_perf_suite)


class RunPerfSuiteTests(unittest.TestCase):
    def test_expand_filters_supports_peers_dimension(self) -> None:
        bench = {
            "templates": ["{impl}/throughput/push_pull/{transport}/peers={peers}/{size}"],
            "sizes": [8192],
            "peers": [8],
        }

        self.assertEqual(
            run_perf_suite.expand_filters(bench, "zmqrs", ["tcp", "ipc"]),
            [
                "zmqrs/throughput/push_pull/ipc/peers=8/8192",
                "zmqrs/throughput/push_pull/tcp/peers=8/8192",
            ],
        )

    def test_standard_profile_selects_omq_key_push_pull_rows(self) -> None:
        config = run_perf_suite.load_config(REPO_ROOT / "perf-suite.json")
        filters = [
            item
            for bench in config["profiles"]["standard"]["benches"]
            for item in run_perf_suite.expand_filters(bench, "zmqrs", ["tcp", "ipc"])
        ]

        self.assertIn("zmqrs/throughput/push_pull/tcp/peers=8/8192", filters)
        self.assertIn("zmqrs/throughput/push_pull/ipc/peers=8/8192", filters)

    def test_duplicate_bench_entries_can_use_distinct_artifact_names(self) -> None:
        self.assertEqual(run_perf_suite.artifact_name({"name": "throughput"}), "throughput")
        self.assertEqual(
            run_perf_suite.artifact_name(
                {"id": "throughput_push_pull_omq_key", "name": "throughput"}
            ),
            "throughput_push_pull_omq_key",
        )


if __name__ == "__main__":
    unittest.main()
