#!/usr/bin/env python3
"""Check service CLI routing and no-change plans without accessing real USB."""
# SPDX-License-Identifier: GPL-3.0-or-later
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / ".work/target/release/turing-screen-monitor"


class ServiceCli(unittest.TestCase):
    def run_cli(self, *args):
        env = os.environ.copy()
        # Dry runs and help must work even without systemctl or sensors on PATH.
        env["PATH"] = ""
        return subprocess.run(
            [str(BINARY), *args], cwd=ROOT, env=env,
            capture_output=True, text=True, timeout=5,
        )

    def test_service_help_bypasses_sensors_and_preview(self):
        for args in [("service",), ("service", "--help"), ("service", "install", "--help")]:
            with self.subTest(args=args):
                result = self.run_cli(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("service install", result.stdout)
                self.assertNotIn("CPU:", result.stderr)
                self.assertNotIn("Preview:", result.stdout)

    def test_install_plan_needs_no_privileges_or_external_commands(self):
        result = self.run_cli("service", "install", "--dry-run", "--enable", "--start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("no files changed and no systemctl commands executed", result.stdout)
        self.assertIn("/usr/local/libexec/turing-screen-monitor", result.stdout)
        self.assertIn("/etc/systemd/system/turing-screen-monitor.service", result.stdout)
        self.assertIn("Request boot enablement: true. Request immediate start: true.", result.stdout)
        self.assertEqual(result.stderr, "")

    def test_uninstall_plan_needs_no_external_commands(self):
        result = self.run_cli("service", "uninstall", "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Stop and disable", result.stdout)
        self.assertIn("preserve the source checkout", result.stdout)
        self.assertEqual(result.stderr, "")

    def test_invalid_arguments_fail_before_any_deployment(self):
        for args in [
            ("service", "unknown"), ("service", "install", "--device", "auto"),
            ("service", "uninstall", "--enable"),
            ("service", "install", "--dry-run", "--dry-run"),
        ]:
            with self.subTest(args=args):
                result = self.run_cli(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("CPU:", result.stderr)
                self.assertNotIn("systemctl ", result.stdout)

    @unittest.skipIf(os.geteuid() == 0, "Never invoke real deployment as root in tests")
    def test_mutations_require_explicit_root_execution(self):
        for action in ["install", "uninstall"]:
            with self.subTest(action=action):
                result = self.run_cli("service", action)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("requires root", result.stderr)
                self.assertNotIn("systemctl ", result.stdout)


if __name__ == "__main__":
    unittest.main()
