# Installation and optional startup

## Repository-local Cargo installation

Build and install as your ordinary user. All tooling, dependencies, temporary
files and the installed executable remain under `.work/`:

```bash
bash tools/bootstrap-rust
bash tools/cargo-local install --path . --locked --root "$PWD/.work/install"
./.work/install/bin/turing-screen-monitor --device auto --interval 2
```

Cargo compiles the application; the destination therefore needs the Rust
toolchain and an existing C linker. Running an already compiled executable does
not require Rust. Reinstall with `--force` after source changes. Removing
`.work/` removes this installation together with the development tools.

Once version 0.1.0 is published, install from crates.io with:

```bash
cargo install turing-screen-monitor --locked
```

The source installation above is available before publication. Both methods
compile the executable and require an existing C linker. Use `--version` to
check the installed binary and `--preview PATH --demo` for a sensor-free preview.

## Service management included in the executable

The executable embeds the systemd template. These commands therefore work after
Cargo installation without requiring the source checkout. Inspect a plan as your
ordinary user; it does not write files, query sensors, access USB or call systemctl:

```bash
./.work/install/bin/turing-screen-monitor service install --dry-run --enable --start
./.work/install/bin/turing-screen-monitor service uninstall --dry-run
```

Real installation/removal requires explicit root execution. Cargo installation,
builds and normal monitoring never install or activate a service automatically.
`service --help` describes the available options.

## Optional root service

The service runs as root so that accessible root-only RAPL counters can supply
CPU package power. It retries a missing USB device, including when the screen
appears after boot. CPU power remains hidden if valid counters are unavailable.
The existing NVIDIA driver and `nvidia-smi` remain runtime requirements for GPU
metrics; the installer does not install them.

Stop any manually running monitor before starting the service. To choose host
installation, build as your ordinary user, inspect the plan and explicitly run:

```bash
bash tools/cargo-local build --release --locked
./.work/target/release/turing-screen-monitor service install --dry-run --enable --start
sudo ./.work/target/release/turing-screen-monitor service install --enable --start
```

The installer copies the running executable to
`/usr/local/libexec/turing-screen-monitor` as root:root with mode 0755 and installs
`/etc/systemd/system/turing-screen-monitor.service` with mode 0644. The service uses
that stable copy, so removing `.work/` cannot break its executable path.

On a first installation, omitting `--enable` leaves boot startup disabled;
omitting `--start` leaves the service stopped. The flags are independent. Each
systemctl action and file change is reported. Inspect operation with:

```bash
systemctl status turing-screen-monitor.service
journalctl -u turing-screen-monitor.service -b
```

Continuous monitoring logs connection events and errors. For local service
settings, use `sudo systemctl edit turing-screen-monitor.service`, then restart the
service. Direct changes to the managed unit file are rejected by the installer;
separate systemd overrides are preserved during updates and removal.

## Updating and removing the service

Cargo updates the Cargo installation, not the executable copy used by the
service. After rebuilding or updating through Cargo, explicitly install that
new executable into the service again:

```bash
sudo ./.work/install/bin/turing-screen-monitor service install
```

An update preserves existing boot enablement and running state. A running service
is stopped before replacement and started with the new executable afterward.
A stopped service stays stopped unless `--start` is supplied. `--enable` can add
boot startup but never implicitly disables an existing installation.

The installer validates directory ownership and permissions, rejects symlinks,
modified units and conflicting installations, and serializes concurrent
installations. Individual file replacements use atomic renames. If a deployment
operation fails, it attempts to restore previous files, modes and enabled/running
state. Any recovery failure is reported as `Rollback incomplete`; inspect the
service before retrying. This rollback handles reported operation errors, not
process termination or power loss midway through deployment.

To stop, disable and remove the managed service files:

```bash
sudo /usr/local/libexec/turing-screen-monitor service uninstall
```

The source checkout, Cargo installation, shared system directories and local
systemd overrides remain. Cargo uninstallation alone does not remove the
separately installed service. Builds and tests never execute host deployment.

## USB access and troubleshooting

The optional root service already has USB access. For manual operation, stop any
competing monitor first and ensure your process can read/write the selected device.
A temporary dialout process can be used without changing permanent membership:

```bash
sudo -u "$USER" -g dialout -- ./.work/target/release/turing-screen-monitor --device auto
```

- An absent device or permission error is reported and retried in continuous
  mode. Check the cable, `/dev/serial/by-id/` and actual process access. No udev
  rules or permissions are installed automatically.
- Multiple detected screens require an explicit device path. Prefer a stable
  by-id path when device numbers can change.
- An unsupported model response is fatal. This release supports only revision A
  3.5-inch displays, including legacy units that do not answer identification.
- `GPU N/A` or query errors: check the existing `nvidia-smi` command and driver.
  A driver installation or upgrade is not performed by this application.
- Hidden CPU power: use a supported, readable RAPL package counter and two valid
  samples at an interval of at most 30 seconds. Root without counters cannot
  provide the metric. See [metric details](metrics.md).
- A modified service unit is rejected by the installer. Keep custom settings in
  systemd overrides, and inspect reported rollback failures before retrying.

## Package and lifecycle validation

```bash
bash tools/cargo-local test --locked service::tests
python3 tests/service_test.py
bash tools/cargo-local package --locked
```

Rust lifecycle tests use temporary destinations and simulated systemctl calls
inside `.work/`. They cover explicit activation, updates, removal, rollback,
concurrent deployment and rejection of unrelated or unsafe destinations. CLI
tests check help, dry runs, invalid options and the root requirement without
accessing USB or changing host files.

Cargo creates and verifies the source archive at
`.work/target/package/turing-screen-monitor-0.1.0.crate`; this does not publish it.
The package includes the embedded service template, license notices, source,
tests and documentation. It excludes development caches and temporary files.
Physical display and service lifecycle validation are separate from these checks.
