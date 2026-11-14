#!/usr/bin/env python3
"""Exercise the release binary using a pseudo terminal; never open real USB."""
# SPDX-License-Identifier: GPL-3.0-or-later
import os
import json
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / ".work/target/release/turing-screen-monitor"


class SerialSmoke(unittest.TestCase):
    def setUp(self):
        self.master, self.slave = pty.openpty()
        self.process = subprocess.Popen(
            [str(BINARY), "--device", os.ttyname(self.slave), "--once"],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )

    def tearDown(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.communicate(timeout=5)
        os.close(self.master)
        os.close(self.slave)

    def read_exact(self, size):
        output = bytearray()
        deadline = time.monotonic() + 8
        while len(output) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.master], [], [], remaining)[0]:
                self.fail(f"Serial timeout: {len(output)}/{size} bytes")
            output.extend(os.read(self.master, size - len(output)))
        return bytes(output)

    def consume_full_frame(self):
        self.assertEqual(self.read_exact(16), bytes([0, 0, 0, 0, 0, 121, 102, 1, 224, 1, 64, 0, 0, 0, 0, 0]))
        self.assertEqual(self.read_exact(6), bytes([51, 0, 0, 0, 0, 110]))  # 20% brightness
        self.assertEqual(self.read_exact(6), bytes([0, 0, 0, 0, 0, 109]))
        self.assertEqual(self.read_exact(6), bytes([0, 0, 7, 125, 63, 197]))
        pixels = self.read_exact(480 * 320 * 2)
        self.assertTrue(any(pixels))

    def assert_full_frame(self):
        self.consume_full_frame()
        _, stderr = self.process.communicate(timeout=5)
        self.assertEqual(self.process.returncode, 0, stderr.decode())
        self.assertIn(b"307206 bytes", stderr)

    def test_identified_35_inch_screen(self):
        self.assertEqual(self.read_exact(6), b"E" * 6)
        os.write(self.master, bytes([1] * 6))
        self.assert_full_frame()

    def test_legacy_screen_without_hello_response(self):
        self.assertEqual(self.read_exact(6), b"E" * 6)
        self.assert_full_frame()

    def test_rejects_unsupported_screen_before_bitmap(self):
        self.assertEqual(self.read_exact(6), b"E" * 6)
        os.write(self.master, bytes([2] * 6))
        _, stderr = self.process.communicate(timeout=5)
        self.assertNotEqual(self.process.returncode, 0)
        self.assertIn(b"Unexpected model response", stderr)
        self.assertFalse(select.select([self.master], [], [], 0)[0])

    def test_profile_counts_frames_and_wire_bytes(self):
        # Replace the initial single-frame process before it opens the pseudo terminal.
        self.process.terminate()
        self.process.communicate(timeout=5)
        temp_root = ROOT / ".work/tmp"
        temp_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=temp_root) as directory:
            profile = Path(directory) / "frames.jsonl"
            self.process = subprocess.Popen(
                [str(BINARY), "--device", os.ttyname(self.slave), "--frames", "2",
                 "--interval", "0.5", "--profile", str(profile)],
                cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            self.assertEqual(self.read_exact(6), b"E" * 6)
            os.write(self.master, bytes([1] * 6))
            self.consume_full_frame()
            subsequent = bytearray()
            deadline = time.monotonic() + 8
            while True:
                if time.monotonic() >= deadline:
                    self.fail("Profiling process did not stop after two frames")
                if select.select([self.master], [], [], 0.05)[0]:
                    subsequent.extend(os.read(self.master, 65536))
                elif self.process.poll() is not None:
                    break
            _, stderr = self.process.communicate(timeout=5)
            self.assertEqual(self.process.returncode, 0, stderr.decode())
            rows = [json.loads(line) for line in profile.read_text().splitlines()]
            self.assertEqual([row["frame"] for row in rows], [1, 2])
            self.assertEqual(rows[0]["bytes"], 307206)
            self.assertEqual(rows[1]["bytes"], len(subsequent))
            offset = 0
            regions = 0
            while offset < len(subsequent):
                header = subsequent[offset:offset + 6]
                self.assertEqual(len(header), 6)
                self.assertEqual(header[5], 197)
                x = (header[0] << 2) | (header[1] >> 6)
                y = ((header[1] & 63) << 4) | (header[2] >> 4)
                right = ((header[2] & 15) << 6) | (header[3] >> 2)
                bottom = ((header[3] & 3) << 8) | header[4]
                self.assertTrue(0 <= x <= right < 480)
                self.assertTrue(0 <= y <= bottom < 320)
                offset += 6 + (right - x + 1) * (bottom - y + 1) * 2
                self.assertLessEqual(offset, len(subsequent))
                regions += 1
            self.assertEqual(offset, len(subsequent))
            self.assertEqual(rows[1]["regions"], regions)
            self.assertGreater(rows[1]["elapsed_s"], rows[0]["elapsed_s"])
            self.assertGreater(rows[0]["rss_kib"], 0)
            self.assertGreaterEqual(rows[1]["self_cpu_s"], rows[0]["self_cpu_s"])
            # A second run cannot destroy the earlier measurements.
            before = profile.read_bytes()
            result = subprocess.run(
                [str(BINARY), "--device", "/nonexistent", "--once", "--profile", str(profile)],
                cwd=ROOT, capture_output=True, timeout=5,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(profile.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
