// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only CPU package power from accessible RAPL energy counters.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct PackageEnergy {
    path: PathBuf,
    energy_uj: u64,
    range_uj: u64,
}

#[derive(Debug)]
struct Sample {
    packages: Vec<PackageEnergy>,
    at: Instant,
}

fn read_number(path: &Path) -> io::Result<u64> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|_| io::Error::other("Invalid RAPL counter"))
}

fn read_packages(root: &Path) -> io::Result<Sample> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)?.flatten() {
        let path = entry.path();
        let name = fs::read_to_string(path.join("name")).unwrap_or_default();
        // Package energy already includes its core/uncore subdomains.
        if name.trim().starts_with("package-") {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "No RAPL packages"));
    }
    let mut packages = Vec::new();
    for path in paths {
        let energy_uj = read_number(&path.join("energy_uj"))?;
        let range_uj = read_number(&path.join("max_energy_range_uj"))?;
        if range_uj == 0 || energy_uj > range_uj {
            return Err(io::Error::other("Invalid RAPL range"));
        }
        packages.push(PackageEnergy {
            path,
            energy_uj,
            range_uj,
        });
    }
    Ok(Sample {
        packages,
        at: Instant::now(),
    })
}

fn watts_between(before: &Sample, after: &Sample) -> Option<f64> {
    let elapsed = after.at.checked_duration_since(before.at)?;
    if elapsed.is_zero()
        || elapsed > MAX_SAMPLE_INTERVAL
        || before.packages.len() != after.packages.len()
        || after.packages.is_empty()
    {
        return None;
    }
    let mut energy_uj = 0_u128;
    for (old, new) in before.packages.iter().zip(&after.packages) {
        if old.path != new.path || old.range_uj != new.range_uj {
            return None;
        }
        let delta = if new.energy_uj >= old.energy_uj {
            new.energy_uj - old.energy_uj
        } else {
            old.range_uj
                .checked_sub(old.energy_uj)?
                .checked_add(new.energy_uj)?
        };
        energy_uj += u128::from(delta);
    }
    let watts = energy_uj as f64 / 1_000_000.0 / elapsed.as_secs_f64();
    watts.is_finite().then_some(watts)
}

#[derive(Debug)]
pub struct RaplMeter {
    roots: Vec<PathBuf>,
    previous: Option<Sample>,
}

impl RaplMeter {
    pub fn new() -> Self {
        Self {
            roots: vec![
                "/sys/class/powercap".into(),
                "/sys/devices/virtual/powercap/intel-rapl".into(),
            ],
            previous: None,
        }
    }

    pub fn sample(&mut self) -> Option<f64> {
        let current = self.roots.iter().find_map(|root| read_packages(root).ok());
        let Some(current) = current else {
            self.previous = None;
            return None;
        };
        let watts = self
            .previous
            .as_ref()
            .and_then(|previous| watts_between(previous, &current));
        self.previous = Some(current);
        watts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_counters_need_two_samples_and_read_failures_reset_the_baseline() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = Fixture(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!(".work/tmp/rapl-{}-{unique}", std::process::id())),
        );
        let package = root.0.join("intel-rapl:0");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("name"), "package-0\n").unwrap();
        fs::write(package.join("energy_uj"), "1000000\n").unwrap();
        fs::write(package.join("max_energy_range_uj"), "100000000\n").unwrap();
        let core = root.0.join("intel-rapl:0:0");
        fs::create_dir_all(&core).unwrap();
        fs::write(core.join("name"), "core\n").unwrap();
        // No core energy file: counting subdomains would fail or double-count.
        let mut meter = RaplMeter {
            roots: vec![root.0.join("missing"), root.0.clone()],
            previous: None,
        };
        assert_eq!(meter.sample(), None);
        assert_eq!(meter.sample(), Some(0.0));
        fs::write(package.join("energy_uj"), "invalid").unwrap();
        assert_eq!(meter.sample(), None);
        assert!(meter.previous.is_none());
        fs::write(package.join("energy_uj"), "1000000\n").unwrap();
        assert_eq!(meter.sample(), None);
        assert_eq!(meter.sample(), Some(0.0));
        fs::remove_file(package.join("energy_uj")).unwrap();
        assert_eq!(meter.sample(), None);
        assert!(meter.previous.is_none());
    }

    fn sample(at: Instant, energy: &[u64]) -> Sample {
        Sample {
            at,
            packages: energy
                .iter()
                .enumerate()
                .map(|(i, energy_uj)| PackageEnergy {
                    path: format!("package-{i}").into(),
                    energy_uj: *energy_uj,
                    range_uj: 100_000_000,
                })
                .collect(),
        }
    }

    #[test]
    fn package_energy_becomes_average_watts_and_wraps_once() {
        let at = Instant::now();
        let before = sample(at, &[10_000_000, 20_000_000]);
        let after = sample(at + Duration::from_secs(2), &[30_000_000, 30_000_000]);
        assert_eq!(watts_between(&before, &after), Some(15.0));
        let before = sample(at, &[90_000_000]);
        let after = sample(at + Duration::from_secs(2), &[10_000_000]);
        assert_eq!(watts_between(&before, &after), Some(10.0));
        assert_eq!(
            watts_between(&before, &sample(at + Duration::from_secs(2), &[90_000_000])),
            Some(0.0)
        );
    }

    #[test]
    fn stale_or_incompatible_samples_do_not_produce_power() {
        let at = Instant::now();
        let before = sample(at, &[10_000_000]);
        assert!(watts_between(&before, &sample(at, &[20_000_000])).is_none());
        assert!(
            watts_between(
                &before,
                &sample(at + Duration::from_secs(31), &[20_000_000])
            )
            .is_none()
        );
        assert!(
            watts_between(
                &before,
                &sample(at + Duration::from_secs(2), &[20_000_000, 1])
            )
            .is_none()
        );
        let mut changed = sample(at + Duration::from_secs(2), &[20_000_000]);
        changed.packages[0].path = "different-package".into();
        assert!(watts_between(&before, &changed).is_none());
        changed.packages[0].path = "package-0".into();
        changed.packages[0].range_uj /= 2;
        assert!(watts_between(&before, &changed).is_none());
    }
}
