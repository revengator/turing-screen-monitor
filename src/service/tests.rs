// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    root: PathBuf,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".work/tmp/service-{}-{id}", std::process::id()));
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::create_dir(&root).unwrap();
        let paths = Paths {
            binary: root.join("usr/local/libexec/turing-screen-monitor"),
            unit: root.join(format!("etc/systemd/system/{NAME}")),
            boundary: root.clone(),
            // SAFETY: geteuid has no arguments or memory safety requirements.
            uid: unsafe { libc::geteuid() },
        };
        for directory in [
            paths.binary.parent().unwrap().parent().unwrap(),
            paths.unit.parent().unwrap(),
        ] {
            fs::create_dir_all(directory).unwrap();
            for parent in directory
                .ancestors()
                .take_while(|path| path.starts_with(&root))
            {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        Self { root, paths }
    }

    fn seed(&self) {
        fs::create_dir_all(self.paths.binary.parent().unwrap()).unwrap();
        // Match the installer's directory mode regardless of the caller's umask.
        fs::set_permissions(
            self.paths.binary.parent().unwrap(),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        atomic_write(
            &self.paths.binary,
            b"previous executable",
            0o750,
            self.paths.uid,
        )
        .unwrap();
        atomic_write(&self.paths.unit, UNIT.as_bytes(), 0o640, self.paths.uid).unwrap();
    }

    fn manager(&self, state: State) -> FakeManager {
        FakeManager {
            paths: (self.paths.binary.clone(), self.paths.unit.clone()),
            state,
            fail: None,
            fail_after: false,
            fail_always: false,
            actions: Vec::new(),
            started_binaries: Vec::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct FakeManager {
    paths: (PathBuf, PathBuf),
    state: State,
    fail: Option<&'static str>,
    fail_after: bool,
    fail_always: bool,
    actions: Vec<String>,
    started_binaries: Vec<Vec<u8>>,
}

impl Manager for FakeManager {
    fn state(&mut self) -> io::Result<State> {
        self.actions.push("state".into());
        Ok(self.state)
    }

    fn apply(&mut self, action: &str) -> io::Result<()> {
        self.actions.push(action.into());
        let fail = self.fail == Some(action);
        if fail && !self.fail_always {
            self.fail = None;
        }
        if fail && !self.fail_after {
            return Err(io::Error::other(format!("Simulated {action} failure")));
        }
        match action {
            "stop" => self.state.running = false,
            "disable" => self.state.enabled = false,
            "enable" => self.state.enabled = true,
            "start" => {
                assert!(self.paths.1.is_file());
                self.started_binaries.push(fs::read(&self.paths.0).unwrap());
                self.state.running = true;
            }
            "daemon-reload" => self.state.loaded = self.paths.1.is_file(),
            _ => panic!("Unexpected simulated action: {action}"),
        }
        if fail {
            return Err(io::Error::other(format!(
                "Simulated {action} failure after change"
            )));
        }
        Ok(())
    }
}

fn install(enable: bool, start: bool) -> Options {
    Options {
        action: Action::Install,
        dry_run: false,
        enable,
        start,
    }
}

fn uninstall() -> Options {
    Options {
        action: Action::Uninstall,
        dry_run: false,
        enable: false,
        start: false,
    }
}

#[test]
fn invalid_service_options_are_rejected() {
    for args in [
        vec![],
        vec!["unknown"],
        vec!["install", "--device", "auto"],
        vec!["uninstall", "--enable"],
        vec!["uninstall", "--start"],
        vec!["install", "--start", "--start"],
        vec!["install", "--dry-run", "--dry-run"],
    ] {
        assert!(Options::parse(args.into_iter().map(String::from)).is_err());
    }
    let options =
        Options::parse(["install", "--enable", "--start", "--dry-run"].map(String::from)).unwrap();
    assert!(options.enable && options.start && options.dry_run);
}

#[test]
fn systemd_state_parser_requires_stable_supported_states() {
    assert_eq!(
        State::parse("LoadState=loaded\nUnitFileState=enabled\nActiveState=active\n").unwrap(),
        State {
            loaded: true,
            enabled: true,
            running: true
        }
    );
    assert_eq!(
        State::parse("LoadState=not-found\nActiveState=inactive\nUnitFileState=\n").unwrap(),
        State::default()
    );
    for text in [
        "",
        "LoadState=loaded\nActiveState=active\n",
        "LoadState=masked\nActiveState=inactive\nUnitFileState=masked\n",
        "LoadState=loaded\nActiveState=activating\nUnitFileState=enabled\n",
        "LoadState=loaded\nActiveState=inactive\nUnitFileState=enabled-runtime\n",
        "LoadState=not-found\nActiveState=active\nUnitFileState=\n",
    ] {
        assert!(State::parse(text).is_err(), "{text}");
    }
}

#[test]
fn fresh_install_needs_explicit_flags_to_enable_or_start() {
    for enable in [false, true] {
        for start in [false, true] {
            let fixture = Fixture::new();
            let mut manager = fixture.manager(State::default());
            deploy(
                &install(enable, start),
                &fixture.paths,
                b"new executable",
                &mut manager,
            )
            .unwrap();
            assert_eq!(
                manager.state,
                State {
                    loaded: true,
                    enabled: enable,
                    running: start
                }
            );
            assert_eq!(fs::read(&fixture.paths.binary).unwrap(), b"new executable");
            assert_eq!(fs::read_to_string(&fixture.paths.unit).unwrap(), UNIT);
            assert_eq!(
                fs::metadata(&fixture.paths.binary).unwrap().mode() & 0o777,
                0o755
            );
            assert_eq!(
                fs::metadata(&fixture.paths.unit).unwrap().mode() & 0o777,
                0o644
            );
        }
    }
}

#[test]
fn updates_preserve_state_and_restart_with_the_new_executable() {
    for enabled in [false, true] {
        for running in [false, true] {
            for enable in [false, true] {
                for start in [false, true] {
                    let fixture = Fixture::new();
                    fixture.seed();
                    let mut manager = fixture.manager(State {
                        loaded: true,
                        enabled,
                        running,
                    });
                    deploy(
                        &install(enable, start),
                        &fixture.paths,
                        b"updated executable",
                        &mut manager,
                    )
                    .unwrap();
                    assert_eq!(
                        manager.state,
                        State {
                            loaded: true,
                            enabled: enabled || enable,
                            running: running || start
                        }
                    );
                    if running || start {
                        assert_eq!(
                            manager.started_binaries.last().unwrap(),
                            b"updated executable"
                        );
                    }
                    assert_eq!(manager.actions.contains(&"stop".into()), running);
                }
            }
        }
    }
}

#[test]
fn uninstall_removes_only_managed_files_and_is_idempotent() {
    let fixture = Fixture::new();
    fixture.seed();
    let unrelated = fixture.root.join("usr/local/keep.txt");
    fs::write(&unrelated, "keep").unwrap();
    let override_directory = fixture.paths.unit.with_extension("service.d");
    fs::create_dir(&override_directory).unwrap();
    fs::write(
        override_directory.join("override.conf"),
        "# Local configuration\n",
    )
    .unwrap();
    let mut manager = fixture.manager(State {
        loaded: true,
        enabled: true,
        running: true,
    });
    deploy(&uninstall(), &fixture.paths, &[], &mut manager).unwrap();
    assert!(!fixture.paths.binary.exists() && !fixture.paths.unit.exists());
    assert_eq!(manager.state, State::default());
    assert!(unrelated.is_file() && override_directory.join("override.conf").is_file());
    assert_eq!(
        manager.actions,
        ["state", "stop", "disable", "daemon-reload"]
    );
    deploy(&uninstall(), &fixture.paths, &[], &mut manager).unwrap();
}

#[test]
fn failed_installation_rolls_back_partial_changes_and_created_directory() {
    for action in ["daemon-reload", "enable", "start"] {
        for fail_after in [false, true] {
            let fixture = Fixture::new();
            let mut manager = fixture.manager(State::default());
            manager.fail = Some(action);
            manager.fail_after = fail_after;
            let error = deploy(
                &install(true, true),
                &fixture.paths,
                b"new executable",
                &mut manager,
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Previous files and enabled/running state restored"),
                "{error}"
            );
            assert!(!fixture.paths.binary.exists() && !fixture.paths.unit.exists());
            assert!(!fixture.paths.binary.parent().unwrap().exists());
            assert_eq!(manager.state, State::default());
        }
    }
}

#[test]
fn failed_update_restores_previous_bytes_modes_and_service_state() {
    for action in ["stop", "daemon-reload", "start"] {
        for fail_after in [false, true] {
            let fixture = Fixture::new();
            fixture.seed();
            let before = State {
                loaded: true,
                enabled: true,
                running: true,
            };
            let mut manager = fixture.manager(before);
            manager.fail = Some(action);
            manager.fail_after = fail_after;
            assert!(
                deploy(
                    &install(false, false),
                    &fixture.paths,
                    b"updated executable",
                    &mut manager
                )
                .is_err()
            );
            assert_eq!(
                fs::read(&fixture.paths.binary).unwrap(),
                b"previous executable"
            );
            assert_eq!(
                fs::metadata(&fixture.paths.binary).unwrap().mode() & 0o777,
                0o750
            );
            assert_eq!(
                fs::metadata(&fixture.paths.unit).unwrap().mode() & 0o777,
                0o640
            );
            assert_eq!(manager.state, before);
            assert_eq!(
                manager.started_binaries.last().unwrap(),
                b"previous executable"
            );
        }
    }
}

#[test]
fn failed_enablement_of_an_existing_disabled_service_preserves_its_state() {
    for running in [false, true] {
        let fixture = Fixture::new();
        fixture.seed();
        let before = State {
            loaded: true,
            enabled: false,
            running,
        };
        let mut manager = fixture.manager(before);
        manager.fail = Some("enable");
        manager.fail_after = true;
        assert!(
            deploy(
                &install(true, true),
                &fixture.paths,
                b"updated executable",
                &mut manager
            )
            .is_err()
        );
        assert_eq!(manager.state, before);
        assert_eq!(
            fs::read(&fixture.paths.binary).unwrap(),
            b"previous executable"
        );
    }
}

#[test]
fn failed_uninstall_restores_files_enablement_and_running_process() {
    for action in ["disable", "daemon-reload"] {
        let fixture = Fixture::new();
        fixture.seed();
        let before = State {
            loaded: true,
            enabled: true,
            running: true,
        };
        let mut manager = fixture.manager(before);
        manager.fail = Some(action);
        manager.fail_after = true;
        assert!(deploy(&uninstall(), &fixture.paths, &[], &mut manager).is_err());
        assert_eq!(manager.state, before);
        assert_eq!(
            fs::read(&fixture.paths.binary).unwrap(),
            b"previous executable"
        );
        assert_eq!(fs::read_to_string(&fixture.paths.unit).unwrap(), UNIT);
    }
}

#[test]
fn rollback_failures_are_reported_without_hiding_the_original_error() {
    let fixture = Fixture::new();
    fixture.seed();
    let mut manager = fixture.manager(State {
        loaded: true,
        enabled: true,
        running: true,
    });
    manager.fail = Some("start");
    manager.fail_always = true;
    let error = deploy(
        &install(false, false),
        &fixture.paths,
        b"updated executable",
        &mut manager,
    )
    .unwrap_err();
    assert!(error.to_string().contains("Simulated start failure"));
    assert!(error.to_string().contains("Rollback incomplete"));
    assert_eq!(
        fs::read(&fixture.paths.binary).unwrap(),
        b"previous executable"
    );
}

#[test]
fn symlinks_and_foreign_or_modified_files_are_rejected_before_stopping() {
    for kind in [
        "unit symlink",
        "binary symlink",
        "parent symlink",
        "dangling parent",
        "foreign unit",
        "modified unit",
        "foreign binary",
        "writable directory",
    ] {
        let fixture = Fixture::new();
        fixture.seed();
        let other = fixture.root.join("keep.txt");
        fs::write(&other, "keep").unwrap();
        match kind {
            "unit symlink" => {
                fs::remove_file(&fixture.paths.unit).unwrap();
                symlink(&other, &fixture.paths.unit).unwrap();
            }
            "binary symlink" => {
                fs::remove_file(&fixture.paths.binary).unwrap();
                symlink(&other, &fixture.paths.binary).unwrap();
            }
            "parent symlink" | "dangling parent" => {
                let parent = fixture.paths.binary.parent().unwrap();
                let moved = fixture.root.join("other");
                fs::rename(parent, &moved).unwrap();
                symlink(
                    if kind == "parent symlink" {
                        moved
                    } else {
                        fixture.root.join("missing")
                    },
                    parent,
                )
                .unwrap();
            }
            "foreign unit" => {
                fs::write(&fixture.paths.unit, "[Service]\nExecStart=/bin/other\n").unwrap()
            }
            "modified unit" => {
                fs::write(&fixture.paths.unit, format!("{UNIT}\n# Custom edits\n")).unwrap()
            }
            "foreign binary" => fs::remove_file(&fixture.paths.unit).unwrap(),
            "writable directory" => fs::set_permissions(
                fixture.paths.unit.parent().unwrap(),
                fs::Permissions::from_mode(0o777),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let mut manager = fixture.manager(State {
            loaded: true,
            enabled: true,
            running: true,
        });
        assert!(
            deploy(
                &install(false, false),
                &fixture.paths,
                b"updated executable",
                &mut manager
            )
            .is_err(),
            "{kind}"
        );
        assert!(!manager.actions.contains(&"stop".into()), "{kind}");
        assert_eq!(fs::read_to_string(other).unwrap(), "keep");
    }
}

#[test]
fn foreign_loaded_unit_and_concurrent_deployment_are_rejected() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager(State {
        loaded: true,
        enabled: false,
        running: false,
    });
    assert!(deploy(&install(false, false), &fixture.paths, b"new", &mut manager).is_err());
    assert!(!fixture.paths.binary.exists());
    let _held_lock = lock(fixture.paths.unit.parent().unwrap()).unwrap();
    manager.actions.clear();
    assert!(deploy(&install(false, false), &fixture.paths, b"new", &mut manager).is_err());
    assert!(manager.actions.is_empty());
}

#[test]
fn atomic_write_failure_keeps_target_and_removes_its_temporary() {
    let fixture = Fixture::new();
    let target = fixture.root.join("directory");
    fs::create_dir(&target).unwrap();
    assert!(atomic_write(&target, b"new", 0o644, fixture.paths.uid).is_err());
    assert!(target.is_dir());
    assert!(
        !fs::read_dir(&fixture.root)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
}
