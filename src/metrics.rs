// SPDX-License-Identifier: GPL-3.0-or-later
use crate::power::RaplMeter;
use std::{
    ffi::CString,
    fs, io,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct CpuTicks {
    idle: u64,
    total: u64,
}

impl CpuTicks {
    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.lines().next()?.split_whitespace();
        if fields.next()? != "cpu" {
            return None;
        }
        // guest and guest_nice are already counted in user and nice.
        let values: Vec<u64> = fields
            .take(8)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        if values.len() < 4 {
            return None;
        }
        Some(Self {
            idle: values[3] + values.get(4).copied().unwrap_or(0),
            total: values.iter().sum(),
        })
    }

    pub fn read() -> io::Result<Self> {
        Self::parse(&fs::read_to_string("/proc/stat")?)
            .ok_or_else(|| io::Error::other("Invalid /proc/stat"))
    }

    fn usage_since(self, before: Self) -> Option<f64> {
        let total = self.total.checked_sub(before.total)?;
        let idle = self.idle.checked_sub(before.idle)?;
        (total > 0 && idle <= total).then(|| 100.0 * (total - idle) as f64 / total as f64)
    }
}

#[derive(Debug, Default)]
pub struct Gpu {
    pub name: String,
    pub usage: Option<f64>,
    pub temperature: Option<f64>,
    pub used_mib: Option<f64>,
    pub total_mib: Option<f64>,
    pub power_watts: Option<f64>,
}

impl Gpu {
    pub fn memory_percentage(&self) -> Option<f64> {
        let used = self.used_mib?;
        let total = self.total_mib?;
        if !used.is_finite() || !total.is_finite() || total <= 0.0 || used < 0.0 || used > total {
            return None;
        }
        Some(used / total * 100.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DiskSpace {
    pub used_gib: f64,
    pub total_gib: f64,
    pub percent: f64,
}

impl DiskSpace {
    fn from_blocks(total: u64, free: u64, available: u64, block_size: u64) -> Option<Self> {
        if total == 0 || block_size == 0 || free > total || available > free {
            return None;
        }
        let bytes = |blocks: u64| u128::from(blocks) * u128::from(block_size);
        Self::from_bytes(bytes(total), bytes(total - free), bytes(available))
    }

    fn from_bytes(total: u128, used: u128, available: u128) -> Option<Self> {
        if total == 0 || used > total || available > total - used {
            return None;
        }
        let usable = used.checked_add(available)?;
        if usable == 0 {
            return None;
        }
        // Match df: round upward and exclude blocks reserved for root.
        let percent = used.checked_mul(100)?.div_ceil(usable) as f64;
        Some(Self {
            used_gib: used as f64 / 1_073_741_824.0,
            total_gib: total as f64 / 1_073_741_824.0,
            percent,
        })
    }
}

#[derive(Debug)]
pub struct Disk {
    pub mount: &'static str,
    pub mounted: bool,
    pub space: Option<DiskSpace>,
}

fn is_mounted(mountinfo: &str, path: &str) -> bool {
    mountinfo
        .lines()
        .any(|line| line.contains(" - ") && line.split_whitespace().nth(4) == Some(path))
}

fn disks() -> io::Result<Vec<Disk>> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(["/"]
        .into_iter()
        .map(|mount| {
            let mounted = is_mounted(&mountinfo, mount);
            Disk {
                mount,
                mounted,
                space: if mounted { disk_space(mount) } else { None },
            }
        })
        .collect())
}

#[derive(Debug)]
pub struct Metrics {
    pub cpu_name: String,
    pub cpu_usage: Option<f64>,
    pub cpu_ghz: Option<f64>,
    pub cpu_temperature: Option<f64>,
    pub cpu_power_watts: Option<f64>,
    pub memory: Option<(f64, f64)>, // used / total GiB; excludes available caches
    pub disks: Vec<Disk>,
    pub uptime: Option<f64>,
    pub gpu: Option<Gpu>,
    pub gpu_status: String,
    previous: CpuTicks,
    gpu_updated: Option<Instant>,
    cpu_power: RaplMeter,
}

pub struct SampleTiming {
    pub total: Duration,
    pub gpu: Duration,
    pub gpu_queried: bool,
}

impl Metrics {
    pub(crate) fn demo() -> Self {
        Self {
            cpu_name: "Example CPU".into(),
            cpu_usage: Some(24.0),
            cpu_ghz: Some(3.2),
            cpu_temperature: Some(45.0),
            cpu_power_watts: None,
            memory: Some((8.0, 32.0)),
            disks: vec![Disk {
                mount: "/",
                mounted: true,
                space: DiskSpace::from_blocks(512, 384, 384, 1_073_741_824),
            }],
            uptime: Some(3600.0),
            gpu: Some(Gpu {
                name: "Example GPU 16 GB".into(),
                usage: Some(32.0),
                temperature: Some(48.0),
                used_mib: Some(4096.0),
                total_mib: Some(16384.0),
                power_watts: Some(65.0),
            }),
            gpu_status: "OK".into(),
            previous: CpuTicks { idle: 0, total: 1 },
            gpu_updated: None,
            cpu_power: RaplMeter::new(),
        }
    }

    pub fn new() -> io::Result<Self> {
        let cpuinfo = fs::read_to_string("/proc/cpuinfo")?;
        let cpu_name = cpuinfo
            .lines()
            .find_map(|s| {
                s.strip_prefix("model name")
                    .and_then(|s| s.split_once(':'))
                    .map(|(_, s)| s.trim().to_string())
            })
            .unwrap_or_else(|| "CPU".into());
        let mut cpu_power = RaplMeter::new();
        cpu_power.sample(); // Establish the first energy reading before warm-up.
        Ok(Self {
            cpu_name,
            cpu_usage: None,
            cpu_ghz: None,
            cpu_temperature: None,
            cpu_power_watts: None,
            memory: None,
            disks: Vec::new(),
            uptime: None,
            gpu: None,
            gpu_status: "Not sampled".into(),
            previous: CpuTicks::read()?,
            gpu_updated: None,
            cpu_power,
        })
    }

    pub fn reset_after_resume(&mut self) -> io::Result<()> {
        self.previous = CpuTicks::read()?;
        self.cpu_usage = None;
        self.cpu_power_watts = None;
        self.cpu_power = RaplMeter::new();
        self.gpu = None;
        self.gpu_updated = None;
        self.gpu_status = "Not sampled after resume".into();
        Ok(())
    }

    pub fn sample(&mut self) -> io::Result<SampleTiming> {
        let started = Instant::now();
        let ticks = CpuTicks::read()?;
        self.cpu_usage = ticks.usage_since(self.previous);
        self.previous = ticks;
        self.cpu_ghz = fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| {
            let mhz: Vec<f64> = s
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("cpu MHz")
                        .and_then(|s| s.split_once(':'))
                        .and_then(|(_, v)| v.trim().parse().ok())
                })
                .collect();
            (!mhz.is_empty()).then(|| mhz.iter().sum::<f64>() / mhz.len() as f64 / 1000.0)
        });
        self.cpu_temperature = cpu_temperature();
        self.cpu_power_watts = self.cpu_power.sample();
        self.memory = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| memory(&s));
        self.disks = disks()?;
        self.uptime = fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse().ok());
        let gpu_started = Instant::now();
        let gpu_queried = self
            .gpu_updated
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
        if gpu_queried {
            match gpu() {
                Ok(gpu) => {
                    self.gpu = Some(gpu);
                    self.gpu_status = "OK".into();
                }
                Err(reason) => {
                    self.gpu = None;
                    self.gpu_status = reason;
                }
            }
            self.gpu_updated = Some(Instant::now());
        }
        Ok(SampleTiming {
            total: started.elapsed(),
            gpu: if gpu_queried {
                gpu_started.elapsed()
            } else {
                Duration::ZERO
            },
            gpu_queried,
        })
    }
}

fn memory(text: &str) -> Option<(f64, f64)> {
    let value = |key: &str| {
        text.lines().find_map(|s| {
            s.strip_prefix(key)
                .and_then(|s| s.split_whitespace().next()?.parse::<u64>().ok())
        })
    };
    let total = value("MemTotal:")?;
    let available = value("MemAvailable:")?;
    if total == 0 || available > total {
        return None;
    }
    Some((
        (total - available) as f64 / 1_048_576.0,
        total as f64 / 1_048_576.0,
    ))
}

fn cpu_temperature() -> Option<f64> {
    for hwmon in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let path = hwmon.path();
        let name = fs::read_to_string(path.join("name")).unwrap_or_default();
        if !matches!(name.trim(), "coretemp" | "k10temp" | "zenpower") {
            continue;
        }
        let mut readings = Vec::new();
        for entry in fs::read_dir(path).ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("temp")
                && name.ends_with("_input")
                && let Ok(value) = fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .trim()
                    .parse::<f64>()
            {
                readings.push(value / 1000.0);
            }
        }
        return readings.into_iter().reduce(f64::max);
    }
    None
}

fn disk_space(path: &str) -> Option<DiskSpace> {
    let path = CString::new(path).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes the structure on success; the path is NUL-terminated.
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    DiskSpace::from_blocks(info.f_blocks, info.f_bfree, info.f_bavail, info.f_frsize)
}

fn gpu() -> Result<Gpu, String> {
    let mut child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,power.draw",
            "--format=csv,noheader,nounits",
        ])
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "nvidia-smi unavailable".to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(1) => {
                thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("NVIDIA query timed out".into());
            }
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("NVIDIA driver/query error".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_gpu_response(&text)
}

fn parse_gpu_response(text: &str) -> Result<Gpu, String> {
    let values: Vec<&str> = text
        .lines()
        .next()
        .ok_or("No NVIDIA GPU")?
        .split(',')
        .map(str::trim)
        .collect();
    if values.len() != 6 {
        return Err("Invalid NVIDIA response".into());
    }
    let number = |i: usize| values[i].parse::<f64>().ok().filter(|v| v.is_finite());
    Ok(Gpu {
        name: values[0].into(),
        usage: number(1),
        temperature: number(2),
        used_mib: number(3),
        total_mib: number(4),
        power_watts: number(5).filter(|v| *v >= 0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_invalidates_cached_gpu_and_cpu_power_baselines() {
        let mut metrics = Metrics::demo();
        metrics.cpu_power_watts = Some(42.0);
        metrics.gpu_updated = Some(Instant::now());
        metrics.reset_after_resume().unwrap();
        assert!(metrics.cpu_usage.is_none());
        assert!(metrics.cpu_power_watts.is_none());
        assert!(metrics.cpu_power.sample().is_none());
        assert!(metrics.gpu.is_none());
        assert!(metrics.gpu_updated.is_none());
        assert_eq!(metrics.gpu_status, "Not sampled after resume");
    }

    #[test]
    fn gpu_power_is_optional_without_hiding_other_metrics() {
        let gpu = parse_gpu_response("Example GPU 16 GB, 17, 42, 512, 16384, 10.25\n").unwrap();
        assert_eq!(gpu.power_watts, Some(10.25));
        assert_eq!(gpu.usage, Some(17.0));
        assert_eq!(gpu.temperature, Some(42.0));
        assert_eq!(gpu.memory_percentage(), Some(3.125));
        for power in ["N/A", "[Not Supported]", "NaN", "inf", "-1"] {
            let gpu =
                parse_gpu_response(&format!("Example GPU 16 GB, 17, 42, 512, 16384, {power}"))
                    .unwrap();
            assert_eq!(gpu.power_watts, None);
            assert_eq!(gpu.usage, Some(17.0));
        }
        assert_eq!(
            parse_gpu_response("GPU, 0, 35, 0, 16384, 0")
                .unwrap()
                .power_watts,
            Some(0.0)
        );
        assert!(parse_gpu_response("GPU, 17, 42, 512, 16384").is_err());
    }
    #[test]
    fn vram_percentage_uses_capacity_and_preserves_missing_data() {
        let mut gpu = Gpu {
            used_mib: Some(4096.0),
            total_mib: Some(16384.0),
            ..Gpu::default()
        };
        assert_eq!(gpu.memory_percentage(), Some(25.0));
        gpu.used_mib = Some(0.0);
        assert_eq!(gpu.memory_percentage(), Some(0.0));
        for (used, total) in [
            (None, Some(16384.0)),
            (Some(5.0), None),
            (Some(0.0), Some(0.0)),
            (Some(-1.0), Some(16.0)),
            (Some(17.0), Some(16.0)),
            (Some(f64::NAN), Some(16.0)),
        ] {
            gpu.used_mib = used;
            gpu.total_mib = total;
            assert_eq!(gpu.memory_percentage(), None);
        }
    }
    #[test]
    fn disks_exclude_reserved_blocks_from_percentage_and_round_up_like_df() {
        let s = DiskSpace::from_blocks(100, 90, 85, 1_073_741_824).unwrap();
        assert_eq!(s.used_gib, 10.0);
        assert_eq!(s.total_gib, 100.0);
        assert_eq!(s.percent, 11.0); // 10 / (10 + 85), rounded upward.
        assert_eq!(
            DiskSpace::from_blocks(1_000_000, 999_999, 950_000, 4096)
                .unwrap()
                .percent,
            1.0
        );
        assert_eq!(
            DiskSpace::from_blocks(100, 100, 95, 4096).unwrap().percent,
            0.0
        );
        assert_eq!(
            DiskSpace::from_blocks(100, 0, 0, 4096).unwrap().percent,
            100.0
        );
        assert!(DiskSpace::from_blocks(0, 0, 0, 4096).is_none());
        assert!(DiskSpace::from_blocks(100, 101, 50, 4096).is_none());
        assert!(DiskSpace::from_blocks(100, 90, 91, 4096).is_none());
    }
    #[test]
    fn root_mount_requires_a_mountinfo_record() {
        let mounts = "44 1 8:1 / / rw shared:1 - ext4 /dev/vda1 rw\n";
        assert!(is_mounted(mounts, "/"));
        assert!(!is_mounted("", "/"));
        assert!(!is_mounted("invalid / record", "/"));
    }
    #[test]
    fn cpu_excludes_guest_and_counts_iowait_as_idle() {
        let before = CpuTicks::parse("cpu 100 0 50 800 50 0 0 0 20 0").unwrap();
        let after = CpuTicks::parse("cpu 150 0 50 840 60 0 0 0 40 0").unwrap();
        assert_eq!(after.usage_since(before), Some(50.0));
        assert_eq!(before.usage_since(after), None);
        assert_eq!(before.usage_since(before), None);
    }
    #[test]
    fn ram_uses_available_including_reclaimable_cache() {
        assert_eq!(
            memory("MemTotal: 8388608 kB\nMemFree: 1 kB\nMemAvailable: 6291456 kB\n"),
            Some((2.0, 8.0))
        );
        assert_eq!(memory("MemTotal: 0 kB\nMemAvailable: 0 kB"), None);
        assert_eq!(memory("MemTotal: 5 kB\nMemAvailable: 6 kB"), None);
        assert_eq!(memory("MemTotal: 5 kB"), None);
    }
}
