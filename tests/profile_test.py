#!/usr/bin/env python3
"""Check that the report includes child CPU and excludes the full startup frame."""
# SPDX-License-Identifier: GPL-3.0-or-later
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parent.parent
summarize = runpy.run_path(str(ROOT / "tools/profile-monitor.py"))["summarize"]


class ProfileSummary(unittest.TestCase):
    def test_startup_and_children_are_accounted_separately(self):
        rows = []
        for index, byte_count in enumerate([300000, 10000, 20000]):
            rows.append({
                "frame": index + 1, "elapsed_s": index + 3,
                "self_cpu_s": 0.5 + index * 0.02,
                "children_cpu_s": 2 + index * 0.1,
                "rss_kib": 2048, "peak_rss_kib": 3072, "child_peak_rss_kib": 16384,
                "bytes": byte_count, "usb_ms": [1800, 60, 120][index],
                "sample_ms": 1, "render_ms": 0.1, "prepare_ms": 0.2,
                "gpu_queried": index != 1, "gpu_ms": 20,
                "voluntary_switches": 100 + index * 10,
            })
        result = summarize(rows)
        self.assertAlmostEqual(result["cpu_percent_one_core"], 2)
        self.assertAlmostEqual(result["children_cpu_percent_one_core"], 10)
        self.assertAlmostEqual(result["total_cpu_percent_one_core"], 12)
        self.assertEqual(result["usb_ms"]["mean"], 90)
        self.assertEqual(result["bytes"]["mean"], 15000)
        self.assertEqual(result["gpu_queries_steady"], 1)
        self.assertEqual(result["parent_peak_rss_mib"], 3)
        self.assertEqual(result["largest_child_peak_rss_mib"], 16)
        self.assertAlmostEqual(result["partial_transfer_fraction_of_full_frame"], 0.05)

    def test_incomplete_measurements_are_rejected(self):
        for rows in ([], [{"frame": 1}], [{"frame": 1}, {"frame": 3}],
                     [{"frame": 1, "elapsed_s": 1}, {"frame": 2, "elapsed_s": 1}]):
            with self.assertRaises(ValueError):
                summarize(rows)


if __name__ == "__main__":
    unittest.main()
