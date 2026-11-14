# Contributor guide

Linux Rust monitor for revision A 3.5-inch USB displays (480 × 320 landscape).

## Structure

`src/metrics.rs` collects sensors, `power.rs` samples RAPL, `render.rs` draws the
panel, `levels.rs` defines colors, `protocol.rs` implements the serial specification
in `docs/protocol.md`, `recovery.rs` handles retries/resume, and `service.rs` manages
explicit systemd deployment. Python integration checks live in `tests/`.

## Development

Use repository-local tooling so toolchains, caches and builds stay in `.work/`:

```bash
bash tools/bootstrap-rust
bash tools/cargo-local build --release --locked
bash tools/cargo-local run --release --locked -- --preview .work/demo.png --demo
bash tools/check-release --allow-dirty
```

The full check runs rustfmt, Clippy, Rust tests, Python pseudo-terminal/recovery/
service/profile checks, packaging and a standalone installation under `.work/`.
Hardware validation is separate; never compete with a monitor holding the display.
Do not change host permissions, packages or services during development checks.

## Style and scope

Use English artifacts, Rust 2024, rustfmt, four-space indentation and snake_case.
Keep GPL-3.0-or-later headers and Cargo.lock. Use concise imperative commit subjects;
PRs describe behavior and validation, with previews for layout changes.

Display CPU usage/frequency/temperature/model/optional package power, NVIDIA GPU
usage/temperature/power/VRAM/model, RAM, the root filesystem `/`, and uptime.
Keep separate GPU load and VRAM gauges. Show GPU POWER in watts or N/A; hide CPU
power unless two valid accessible RAPL samples exist. Missing values stay unavailable.
The root disk has one gauge and used/total capacity text in MiB/GiB/TiB as needed.
Do not add unrelated indicators, branding or decorative text.

Use green/amber/red for LOW/MEDIUM/HIGH and gray for unavailable values. Section
headers show the highest known metric level. Frequency, capacity, uptime and power
stay neutral. Thresholds are visual conventions, documented in `docs/metrics.md`,
rather than hardware safety limits. Preserve exact protocol wire vectors in tests.
