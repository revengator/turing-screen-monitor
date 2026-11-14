# Development

## Tools and layout

Use the repository-local Rust wrappers; they keep Rustup, Cargo, build outputs,
caches and temporary files in `.work/`. Bash, curl, an existing C linker and Python
3.11 or newer are needed for the complete check suite. Python checks use the standard library;
the suspend simulation compiles a small C helper with the existing linker.

```bash
bash tools/bootstrap-rust
bash tools/cargo-local build --release --locked
bash tools/check-release --allow-dirty
```

Rust is pinned in `rust-toolchain.toml` and dependencies in `Cargo.lock`. Do not
build as root or install packages globally. Four-space indentation, rustfmt and
Clippy apply. All artifacts must be in English. Keep sensor collection, drawing,
serial communication and service management separate.

## Individual checks

```bash
bash tools/cargo-local fmt --check
bash tools/cargo-local clippy --locked --all-targets -- -D warnings
bash tools/cargo-local test --locked
bash tools/cargo-local build --release --locked
python3 -B tests/serial_smoke.py
python3 -B tests/recovery_test.py
python3 -B tests/service_test.py
python3 -B tests/profile_test.py
```

Serial and recovery checks use pseudo terminals, never the physical screen.
The clock interposer changes time only inside the test process. Service tests
use temporary destinations and simulated systemctl operations. Real host service
installation, update/removal, USB unplugging and suspend are separate checks.

Calculations, missing metrics, serial packets, rendering and service rollback
have unit coverage. No coverage percentage is enforced. Add regression tests for
behavioral fixes rather than tests duplicating simple implementation details.

Regenerate the documented preview with fixed example metrics:

```bash
./.work/target/release/turing-screen-monitor --preview assets/panel-preview.png --demo
```

Generate additional offline visual examples:

```bash
TURING_TEST_PREVIEW_DIR="$PWD/.work/color-previews" bash tools/cargo-local test --locked
```

## Physical hardware and profiling

Use a display that is not held by another process. Do not run a competing monitor
or benchmark while a service owns the port.
Preview mode does not need USB access. Missing sensor data must remain unavailable
rather than becoming invented zeros.

With access to an unused display:

```bash
python3 tools/profile-monitor.py --frames 60 --interval 2
python3 tools/profile-monitor.py --frames 60 --interval 2 --full-width-updates
```

Measurements and logs stay in `.work/profiles/`. CPU accounting includes the monitor
and separately reaped children; 100% means one logical CPU. Child peak RSS is not
simultaneous process-tree memory.

## Contributions

Use concise imperative commit subjects. Describe behavior, validation and related
issues in pull requests; include a preview for visible layout changes. Preserve
license headers. Use generic sample hardware names and deterministic demo previews.
Do not commit credentials or include local tooling in the source package.
