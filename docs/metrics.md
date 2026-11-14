# Metrics and display behavior

The panel uses CPU/GPU at the top, RAM/root disk below and uptime at the bottom.
Only labels identifying hardware, measurements, units and uptime are displayed.

## Collection and unavailable data

CPU utilization uses differences in `/proc/stat` counters; iowait counts as idle
and guest counters are not counted twice. Clock frequency averages the available
`cpu MHz` entries. Temperature uses the maximum reading from a supported CPU
hwmon device (`coretemp`, `k10temp` or `zenpower`).

RAM uses `MemTotal - MemAvailable` from `/proc/meminfo`. Reclaimable caches are
therefore treated differently from some other monitoring tools. Uptime comes
from `/proc/uptime`.

NVIDIA collection uses the first GPU returned by one `nvidia-smi` query. The query
includes utilization, temperature, used/total VRAM and `power.draw`, times out
after one second and runs at most once every five seconds. With a two-second
refresh interval it usually runs every six seconds. Query errors clear the
cached GPU values. Unsupported GPU power shows `POWER N/A W` without discarding
other available readings.

VRAM occupancy is used memory divided by total memory. GPU processing utilization
and VRAM occupancy always have separate gauges. Temperature, used/total VRAM and
the GPU header's power field remain visible.

## Root filesystem

Only `/` is collected. It must have an exact mount record in
`/proc/self/mountinfo`. A missing mount record or failed filesystem query gives N/A.
The disk panel has one root-usage gauge and used/total capacity text.

The percentage matches `df`: used / (used + available space), rounded upward.
Displayed total capacity includes reserved space. Capacity text uses MiB, GiB or
TiB according to magnitude. No other mount points are collected or configured.

## Optional CPU power

CPU package power appears only after two valid, readable RAPL energy readings.
The decision depends on access, not UID. Root without suitable counters cannot
supply a reading; an authorized ordinary user with access can.

Package counters are summed without core/uncore subdomains, avoiding double
counting. Microjoule deltas are divided by elapsed seconds to obtain watts.
Single counter wraps use `max_energy_range_uj`. Read failures, changed package
sets/ranges, zero-duration samples and gaps over 30 seconds reset or invalidate
the measurement. Use an interval of at most 30 seconds for CPU power.

This is average package power over the interval, not total wall power. CPU power
is hidden when unavailable; GPU power instead remains visible as `N/A`.
Running the compiled application as root is optional. It does not require
changing counter permissions or installing a service.

## Colors and textual levels

Green means `LOW`, amber `MEDIUM`, red `HIGH` and gray unavailable (`N/A`).
Classification uses the same rounding as the displayed value.

| Metric | Low | Medium | High |
| --- | --- | --- | --- |
| CPU/GPU processing load | <50% | 50–<80% | ≥80% |
| RAM/VRAM occupancy | <60% | 60–<85% | ≥85% |
| Disk usage | <70% | 70–<90% | ≥90% |
| CPU temperature | <60 °C | 60–<80 °C | ≥80 °C |
| GPU temperature | <55 °C | 55–<75 °C | ≥75 °C |

Headers and borders show the highest known level within the section. Individual
metric colors still reflect their own readings. Frequency, capacities, uptime
and power remain neutral; no wattage severity limits are inferred.

These are visual conventions in `src/levels.rs`, not manufacturer safety limits.
Document any changes here before changing their meaning.

## USB transfers and recovery

The first image is complete. Later frames send only rectangles bounded by changed
pixels; no frame queue builds up. `--full-width-updates` reproduces the previous
full-width row transfer policy for measurements.

Continuous monitoring retries after 1, 2, 4, 8 and then at most 15 seconds.
A successful frame resets the delay. Auto detection runs on every attempt;
explicit paths should preferably use `/dev/serial/by-id/...`. Unsupported models
remain fatal. `--once`, `--frames` and `--profile` fail immediately on serial errors.

Recovery discards the previous descriptor and cached image, repeats identification,
orientation and brightness, and sends a full frame from fresh samples.
The difference between Linux `CLOCK_BOOTTIME` and `CLOCK_MONOTONIC` detects
suspend longer than 100 ms independently of wall-clock changes. Resume resets
CPU/RAPL baselines and the GPU cache. CPU power waits for two new valid samples.
An active serial operation may finish or time out before shutdown or recovery.
See the [Linux clock documentation](https://man7.org/linux/man-pages/man3/clock_gettime.3.html).
