#!/usr/bin/env python3
"""Measure the release binary on real hardware; use only the Python standard library."""
# SPDX-License-Identifier: GPL-3.0-or-later
import argparse
from datetime import datetime, timezone
import json
import hashlib
from pathlib import Path
import platform
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def summarize(rows):
    if len(rows) < 2:
        raise ValueError("At least two frames are needed to separate startup from steady operation")
    if [row["frame"] for row in rows] != list(range(1, len(rows) + 1)):
        raise ValueError("Incomplete or unordered frame measurements")
    first, last = rows[0], rows[-1]
    duration = last["elapsed_s"] - first["elapsed_s"]
    if duration <= 0:
        raise ValueError("The measured duration must be positive")
    steady = rows[1:]
    own_cpu = (last["self_cpu_s"] - first["self_cpu_s"]) / duration * 100
    child_cpu = (last["children_cpu_s"] - first["children_cpu_s"]) / duration * 100
    result = {
        "frames": len(rows), "steady_frames": len(steady),
        "steady_duration_s": duration,
        "cpu_percent_one_core": own_cpu,
        "children_cpu_percent_one_core": child_cpu,
        "total_cpu_percent_one_core": own_cpu + child_cpu,
        "parent_rss_median_mib": statistics.median(row["rss_kib"] for row in steady) / 1024,
        "parent_peak_rss_mib": max(row["peak_rss_kib"] for row in rows) / 1024,
        "largest_child_peak_rss_mib": max(row["child_peak_rss_kib"] for row in rows) / 1024,
        "startup_frame_bytes": first["bytes"], "startup_usb_ms": first["usb_ms"],
        "gpu_queries_steady": sum(row["gpu_queried"] for row in steady),
        "voluntary_switches_per_second": (last["voluntary_switches"] - first["voluntary_switches"]) / duration,
        "partial_transfer_fraction_of_full_frame": statistics.mean(row["bytes"] for row in steady) / first["bytes"],
    }
    for key in ("sample_ms", "render_ms", "prepare_ms", "usb_ms", "bytes"):
        values = sorted(row[key] for row in steady)
        result[key] = {"mean": statistics.mean(values), "median": statistics.median(values),
                       "min": values[0], "max": values[-1]}
    queries = [row["gpu_ms"] for row in steady if row["gpu_queried"]]
    result["gpu_query_ms_mean"] = statistics.mean(queries) if queries else None
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default="auto")
    parser.add_argument("--frames", type=int, default=60)
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / ".work/target/release/turing-screen-monitor")
    parser.add_argument("--full-width-updates", action="store_true", help="Compare the legacy transfer policy")
    args = parser.parse_args()
    if args.frames < 2 or not 0.5 <= args.interval <= 3600:
        parser.error("Use at least two frames and an interval of 0.5..3600 seconds")
    output = args.output or ROOT / ".work/profiles" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    command = [str(args.binary.resolve()), "--device", args.device, "--interval", str(args.interval),
               "--frames", str(args.frames), "--profile", str(output / "frames.jsonl")]
    if args.full_width_updates:
        command.append("--full-width-updates")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    context = {"date_utc": datetime.now(timezone.utc).isoformat(), "kernel": platform.release(),
               "revision": revision, "dirty_worktree": dirty, "command": command,
               "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
               "cpu_basis": "100% = one logical CPU; children counted separately",
               "memory_basis": "Parent VmRSS/VmHWM; child peak = largest reaped child, not total tree RSS"}
    (output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
    print(f"Measuring {args.frames} frames, interval {args.interval}s. Results: {output}", flush=True)
    with (output / "monitor.log").open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=log)
        try:
            code = process.wait(timeout=args.frames * (args.interval + 4) + 30)
        except BaseException:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            raise
    if code != 0:
        raise RuntimeError(f"Monitor exited with {code}; see {output / 'monitor.log'}")
    rows = [json.loads(line) for line in (output / "frames.jsonl").read_text().splitlines()]
    if len(rows) != args.frames:
        raise RuntimeError(f"Expected {args.frames} frames; received {len(rows)}")
    result = summarize(rows)
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
