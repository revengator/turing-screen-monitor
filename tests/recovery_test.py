#!/usr/bin/env python3
"""Test USB recovery through replaceable pseudo terminals, without real USB."""
# SPDX-License-Identifier: GPL-3.0-or-later
import os
from pathlib import Path
import pty
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest

sys.dont_write_bytecode = True
import serial_smoke as wire


class RecoveryTests(unittest.TestCase):
    read_exact = wire.SerialSmoke.read_exact
    consume_full_frame = wire.SerialSmoke.consume_full_frame

    def setUp(self):
        root = wire.ROOT / ".work/tmp"
        root.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=root)
        self.directory = Path(self.temporary.name)
        self.device = self.directory / "display"
        self.log_path = self.directory / "monitor.log"
        self.log = self.log_path.open("wb")
        self.process = None
        self.master = self.slave = None

    def tearDown(self):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.kill()
            self.process.wait(timeout=5)
        self.disconnect()
        self.log.close()
        self.temporary.cleanup()

    def start(self, *extra, env=None):
        self.process = subprocess.Popen(
            [str(wire.BINARY), "--device", str(self.device), "--interval", "0.5", *extra],
            cwd=wire.ROOT, stdout=self.log, stderr=self.log, env=env,
        )

    def attach(self):
        self.master, self.slave = pty.openpty()
        self.device.symlink_to(os.ttyname(self.slave))

    def disconnect(self):
        self.device.unlink(missing_ok=True)
        for name in ("master", "slave"):
            descriptor = getattr(self, name)
            if descriptor is not None:
                os.close(descriptor)
                setattr(self, name, None)

    def handshake(self):
        self.assertEqual(self.read_exact(6), b"E" * 6)
        os.write(self.master, bytes([1] * 6))

    def wait_log(self, message):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            log = self.log_path.read_text()
            if message in log:
                return log
            if self.process.poll() is not None:
                self.fail(f"Monitor exited before {message!r}: {log}")
            time.sleep(0.02)
        self.fail(f"Missing log message {message!r}: {self.log_path.read_text()}")

    def stop(self, signum=signal.SIGTERM):
        self.process.send_signal(signum)
        self.assertEqual(self.process.wait(timeout=4), 0, self.log_path.read_text())

    def test_waits_for_missing_device_then_sends_full_frame(self):
        self.start()
        self.wait_log("USB unavailable")
        self.attach()
        self.handshake()
        self.consume_full_frame()
        self.stop()
        self.assertNotIn(" bytes in ", self.log_path.read_text())

    def test_disconnect_after_success_discards_cached_frame(self):
        self.attach()
        self.start("--verbose")
        self.handshake()
        self.consume_full_frame()
        self.wait_log("307206 bytes")
        self.disconnect()
        self.wait_log("USB transfer failed")
        self.attach()
        self.handshake()
        self.consume_full_frame()
        self.stop(signal.SIGINT)
        self.assertEqual(self.log_path.read_text().count("Connected to"), 2)

    def test_disconnect_midway_through_full_transfer_reinitializes(self):
        self.attach()
        self.start()
        self.handshake()
        self.read_exact(16 + 6 + 6)  # Initialization.
        self.assertEqual(self.read_exact(6), bytes([0, 0, 7, 125, 63, 197]))
        self.read_exact(1024)
        self.disconnect()
        self.wait_log("USB transfer failed")
        self.attach()
        self.handshake()
        self.consume_full_frame()
        self.stop()

    def test_sigterm_interrupts_backoff_without_waiting_for_retry(self):
        self.start()
        self.wait_log("Retrying in 2 s")
        started = time.monotonic()
        self.stop()
        self.assertLess(time.monotonic() - started, 1)

    def test_unsupported_model_is_fatal_in_continuous_mode(self):
        self.attach()
        self.start()
        self.assertEqual(self.read_exact(6), b"E" * 6)
        os.write(self.master, bytes([2] * 6))
        self.assertNotEqual(self.process.wait(timeout=5), 0)
        self.assertIn("Unexpected model response", self.log_path.read_text())
        self.assertNotIn("Retrying", self.log_path.read_text())
        self.assertFalse(select.select([self.master], [], [], 0)[0])

    def test_bounded_runs_fail_when_device_is_missing(self):
        for args in [("--once",), ("--frames", "2"),
                     ("--profile", str(self.directory / "frames.jsonl"))]:
            with self.subTest(args=args):
                result = subprocess.run(
                    [str(wire.BINARY), "--device", str(self.device), *args],
                    cwd=wire.ROOT, capture_output=True, timeout=5,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(b"Retrying", result.stderr)

    def test_simulated_resume_reopens_port_and_sends_full_frame(self):
        library = self.directory / "suspend-clock.so"
        subprocess.run(
            ["cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
             str(wire.ROOT / "tests/suspend_clock.c"), "-ldl", "-o", str(library)],
            check=True, capture_output=True,
        )
        marker = self.directory / "resume"
        env = os.environ.copy()
        env["TURING_TEST_SUSPEND_FILE"] = str(marker)
        env["LD_PRELOAD"] = str(library)
        self.attach()
        self.start("--interval", "3600", env=env)
        self.handshake()
        self.consume_full_frame()
        marker.touch()
        self.wait_log("System resumed; resetting sensor baselines")
        self.handshake()
        self.consume_full_frame()
        self.stop()
        self.assertEqual(self.log_path.read_text().count("Connected to"), 2)


if __name__ == "__main__":
    unittest.main()
