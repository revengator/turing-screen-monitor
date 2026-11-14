// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit system service deployment, separate from monitoring and Cargo installation.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const NAME: &str = "turing-screen-monitor.service";
const UNIT: &str = include_str!("../contrib/systemd/turing-screen-monitor.service");
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const HELP: &str = "\
turing-screen-monitor service install [--dry-run] [--enable] [--start]
turing-screen-monitor service uninstall [--dry-run]

Install the compiled executable and embedded systemd unit, or remove them.
--dry-run prints the plan without root access, file changes or systemctl calls.
--enable opts into startup at boot; --start opts into starting the service now.
Updates preserve the existing enabled/running state unless these flags add to it.
Actual installation/removal requires root. Compile and install with Cargo as your user.
Stop a manually running monitor before starting the system service.";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Install,
    Uninstall,
}

#[derive(Debug)]
struct Options {
    action: Action,
    dry_run: bool,
    enable: bool,
    start: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> io::Result<Self> {
        let mut args = args.into_iter();
        let action = match args.next().as_deref() {
            Some("install") => Action::Install,
            Some("uninstall") => Action::Uninstall,
            _ => {
                return Err(io::Error::other(
                    "Expected service install or service uninstall; use service --help",
                ));
            }
        };
        let mut options = Self {
            action,
            dry_run: false,
            enable: false,
            start: false,
        };
        for arg in args {
            match arg.as_str() {
                "--dry-run" if !options.dry_run => options.dry_run = true,
                "--enable" if action == Action::Install && !options.enable => options.enable = true,
                "--start" if action == Action::Install && !options.start => options.start = true,
                _ => {
                    return Err(io::Error::other(format!(
                        "Invalid or repeated service option: {arg}"
                    )));
                }
            }
        }
        Ok(options)
    }
}

struct Paths {
    binary: PathBuf,
    unit: PathBuf,
    boundary: PathBuf,
    uid: u32,
}

impl Paths {
    fn system() -> Self {
        Self {
            binary: "/usr/local/libexec/turing-screen-monitor".into(),
            unit: format!("/etc/systemd/system/{NAME}").into(),
            boundary: "/".into(),
            uid: 0,
        }
    }

    fn check_directory(&self, path: &Path) -> io::Result<()> {
        if !path.starts_with(&self.boundary) {
            return Err(io::Error::other("Service destination escaped its boundary"));
        }
        let mut current = path;
        loop {
            let metadata = fs::symlink_metadata(current)?;
            if !metadata.is_dir() || metadata.uid() != self.uid || metadata.mode() & 0o022 != 0 {
                return Err(io::Error::other(format!(
                    "Refusing untrusted service directory: {}",
                    current.display()
                )));
            }
            if current == self.boundary {
                return Ok(());
            }
            current = current
                .parent()
                .ok_or_else(|| io::Error::other("Invalid service directory"))?;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct State {
    loaded: bool,
    enabled: bool,
    running: bool,
}

impl State {
    fn parse(text: &str) -> io::Result<Self> {
        let value = |key: &str| {
            text.lines().find_map(|line| {
                let (name, value) = line.split_once('=')?;
                (name == key).then_some(value)
            })
        };
        let loaded = match value("LoadState") {
            Some("loaded") => true,
            Some("not-found") => false,
            _ => {
                return Err(io::Error::other(
                    "Unsupported service load state; no changes made",
                ));
            }
        };
        let enabled = match value("UnitFileState") {
            Some("enabled") => true,
            Some("disabled" | "") => false,
            _ => {
                return Err(io::Error::other(
                    "Unsupported service enablement state; no changes made",
                ));
            }
        };
        let running = match value("ActiveState") {
            Some("active") => true,
            Some("inactive" | "failed") => false,
            _ => {
                return Err(io::Error::other(
                    "Service is transitioning; retry when its state is stable",
                ));
            }
        };
        if !loaded && (enabled || running) {
            return Err(io::Error::other(
                "Inconsistent service state; no changes made",
            ));
        }
        Ok(Self {
            loaded,
            enabled,
            running,
        })
    }
}

trait Manager {
    fn state(&mut self) -> io::Result<State>;
    fn apply(&mut self, action: &str) -> io::Result<()>;
}

struct Systemd;

impl Systemd {
    fn command(args: &[&str]) -> io::Result<String> {
        let output = Command::new(SYSTEMCTL)
            .arg("--system")
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C")
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "systemctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl Manager for Systemd {
    fn state(&mut self) -> io::Result<State> {
        State::parse(&Self::command(&[
            "show",
            "--property=LoadState",
            "--property=ActiveState",
            "--property=UnitFileState",
            NAME,
        ])?)
    }

    fn apply(&mut self, action: &str) -> io::Result<()> {
        println!(
            "systemctl {action}{}",
            if action == "daemon-reload" {
                ""
            } else {
                " turing-screen-monitor.service"
            }
        );
        if action == "daemon-reload" {
            Self::command(&[action])?;
        } else {
            Self::command(&[action, NAME])?;
        }
        Ok(())
    }
}

struct Snapshot {
    bytes: Vec<u8>,
    mode: u32,
}

fn snapshot(path: &Path, uid: u32) -> io::Result<Option<Snapshot>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o7022 != 0 {
        return Err(io::Error::other(format!(
            "Refusing untrusted service file: {}",
            path.display()
        )));
    }
    Ok(Some(Snapshot {
        bytes: fs::read(path)?,
        mode: metadata.mode() & 0o777,
    }))
}

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &Path, bytes: &[u8], mode: u32, uid: u32) -> io::Result<()> {
    let temporary = path.with_file_name(format!(
        ".turing-screen-monitor-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    let result = (|| {
        if uid == 0 {
            // SAFETY: file holds a valid descriptor; the actual installer requires root.
            if unsafe { libc::fchown(file.as_raw_fd(), 0, 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn restore(path: &Path, before: &Option<Snapshot>, uid: u32) -> io::Result<()> {
    match before {
        Some(snapshot) => atomic_write(path, &snapshot.bytes, snapshot.mode, uid),
        None => match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        },
    }
}

fn lock(directory: &Path) -> io::Result<File> {
    let file = File::open(directory)?;
    // SAFETY: the directory descriptor remains open for the duration of the deployment.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other(
            "Another service deployment is in progress",
        ));
    }
    Ok(file)
}

fn rollback(
    paths: &Paths,
    manager: &mut impl Manager,
    state: State,
    binary: &Option<Snapshot>,
    unit: &Option<Snapshot>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let current = match manager.state() {
        Ok(current) => Some(current),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    let mut attempt = |result: io::Result<()>| {
        if let Err(error) = result {
            errors.push(error.to_string());
        }
    };
    if current.map_or_else(|| paths.unit.exists(), |state| state.running) {
        attempt(manager.apply("stop"));
    }
    if current.map_or_else(|| paths.unit.exists(), |state| state.enabled) {
        attempt(manager.apply("disable"));
    }
    attempt(restore(&paths.binary, binary, paths.uid));
    attempt(restore(&paths.unit, unit, paths.uid));
    attempt(manager.apply("daemon-reload"));
    if state.enabled {
        attempt(manager.apply("enable"));
    }
    if state.running {
        attempt(manager.apply("start"));
    }
    errors
}

fn deploy(
    options: &Options,
    paths: &Paths,
    source: &[u8],
    manager: &mut impl Manager,
) -> io::Result<()> {
    let unit_parent = paths.unit.parent().unwrap();
    let binary_parent = paths.binary.parent().unwrap();
    paths.check_directory(unit_parent)?;
    let _lock = lock(unit_parent)?;
    let create_parent = match fs::symlink_metadata(binary_parent) {
        Ok(_) => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    paths.check_directory(if create_parent {
        binary_parent.parent().unwrap()
    } else {
        binary_parent
    })?;
    // Check the destination and its ownership before stopping anything.
    let binary = snapshot(&paths.binary, paths.uid)?;
    let unit = snapshot(&paths.unit, paths.uid)?;
    if unit
        .as_ref()
        .is_some_and(|unit| unit.bytes != UNIT.as_bytes())
    {
        return Err(io::Error::other(
            "Existing service unit differs from the embedded template; preserve custom settings with systemctl edit",
        ));
    }
    if unit.is_none() && binary.is_some() {
        return Err(io::Error::other(
            "An unmanaged executable occupies the service destination; no changes made",
        ));
    }
    let state = manager.state()?;
    if unit.is_none() && state.loaded {
        return Err(io::Error::other(
            "A service from another location already uses this name; no changes made",
        ));
    }
    if options.action == Action::Uninstall && unit.is_none() {
        println!("Service is not installed.");
        return Ok(());
    }
    let mut created_parent = false;
    let result: io::Result<()> = (|| {
        if state.running || (options.action == Action::Uninstall && state.loaded) {
            manager.apply("stop")?;
        }
        match options.action {
            Action::Install => {
                if create_parent {
                    fs::create_dir(binary_parent)?;
                    created_parent = true;
                    fs::set_permissions(binary_parent, fs::Permissions::from_mode(0o755))?;
                }
                println!("Install executable: {}", paths.binary.display());
                atomic_write(&paths.binary, source, 0o755, paths.uid)?;
                println!("Install systemd unit: {}", paths.unit.display());
                atomic_write(&paths.unit, UNIT.as_bytes(), 0o644, paths.uid)?;
                manager.apply("daemon-reload")?;
                if options.enable && !state.enabled {
                    manager.apply("enable")?;
                }
                if options.start || state.running {
                    manager.apply("start")?;
                }
            }
            Action::Uninstall => {
                if state.enabled {
                    manager.apply("disable")?;
                }
                println!("Remove executable: {}", paths.binary.display());
                restore(&paths.binary, &None, paths.uid)?;
                println!("Remove systemd unit: {}", paths.unit.display());
                fs::remove_file(&paths.unit)?;
                manager.apply("daemon-reload")?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = rollback(paths, manager, state, &binary, &unit);
        if created_parent && let Err(error) = fs::remove_dir(binary_parent) {
            failures.push(error.to_string());
        }
        let status = if failures.is_empty() {
            "Previous files and enabled/running state restored.".into()
        } else {
            format!(
                "Rollback incomplete: {}. Inspect the service before retrying.",
                failures.join("; ")
            )
        };
        return Err(io::Error::other(format!(
            "Service deployment failed: {error}. {status}"
        )));
    }
    Ok(())
}

fn plan(options: &Options, paths: &Paths) {
    println!("Dry run: no files changed and no systemctl commands executed.");
    match options.action {
        Action::Install => {
            println!(
                "Install the running executable as root:root, mode 0755: {}",
                paths.binary.display()
            );
            println!(
                "Install the embedded systemd unit as root:root, mode 0644: {}",
                paths.unit.display()
            );
            println!(
                "Reload systemd; preserve existing enabled/running state and restart a running service."
            );
            println!(
                "Request boot enablement: {}. Request immediate start: {}.",
                options.enable, options.start
            );
        }
        Action::Uninstall => {
            println!("Stop and disable {NAME}; remove only these managed files:");
            println!("{}", paths.binary.display());
            println!("{}", paths.unit.display());
            println!(
                "Reload systemd; preserve the source checkout, Cargo installation and local overrides."
            );
        }
    }
    println!(
        "Real deployment checks ownership, rejects symlinks/modified units, and rolls back errors."
    );
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = args.into_iter().collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let options = Options::parse(args)?;
    let paths = Paths::system();
    if options.dry_run {
        plan(&options, &paths);
        return Ok(());
    }
    // SAFETY: geteuid has no arguments or memory safety requirements.
    if unsafe { libc::geteuid() } != 0 {
        return Err("Service installation/removal requires root; use --dry-run to inspect the plan without changes".into());
    }
    let source = if options.action == Action::Install {
        fs::read("/proc/self/exe")?
    } else {
        Vec::new()
    };
    deploy(&options, &paths, &source, &mut Systemd)?;
    println!(
        "Service {} complete.",
        if options.action == Action::Install {
            "installation"
        } else {
            "removal"
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests;
