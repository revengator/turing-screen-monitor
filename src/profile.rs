// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional JSONL measurements. Child RSS is the largest reaped child, not tree RSS.
use crate::{metrics::SampleTiming, protocol::UpdateStats};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};

fn usage(who: libc::c_int) -> io::Result<libc::rusage> {
    let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // getrusage initializes the structure on success.
    if unsafe { libc::getrusage(who, value.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { value.assume_init() })
}

fn cpu_seconds(usage: &libc::rusage) -> f64 {
    let time = |value: libc::timeval| value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0;
    time(usage.ru_utime) + time(usage.ru_stime)
}

pub struct Profile {
    writer: BufWriter<File>,
    started: Instant,
    self_start: f64,
    child_start: f64,
}

impl Profile {
    pub fn new(path: &Path) -> io::Result<Self> {
        let self_start = cpu_seconds(&usage(libc::RUSAGE_SELF)?);
        let child_start = cpu_seconds(&usage(libc::RUSAGE_CHILDREN)?);
        // Never silently overwrite measurements from an earlier run.
        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            started: Instant::now(),
            self_start,
            child_start,
        })
    }

    pub fn record(
        &mut self,
        frame: usize,
        sample: &SampleTiming,
        render: Duration,
        update: &UpdateStats,
    ) -> io::Result<()> {
        let own = usage(libc::RUSAGE_SELF)?;
        let children = usage(libc::RUSAGE_CHILDREN)?;
        let status = fs::read_to_string("/proc/self/status")?;
        let kib = |key: &str| -> io::Result<u64> {
            status
                .lines()
                .find_map(|line| {
                    line.strip_prefix(key)?
                        .split_whitespace()
                        .next()?
                        .parse()
                        .ok()
                })
                .ok_or_else(|| io::Error::other(format!("Missing {key} in /proc/self/status")))
        };
        writeln!(
            self.writer,
            "{{\"frame\":{},\"elapsed_s\":{:.6},\"sample_ms\":{:.6},\"gpu_ms\":{:.6},\"gpu_queried\":{},\"render_ms\":{:.6},\"prepare_ms\":{:.6},\"usb_ms\":{:.6},\"bytes\":{},\"regions\":{},\"self_cpu_s\":{:.6},\"children_cpu_s\":{:.6},\"rss_kib\":{},\"peak_rss_kib\":{},\"child_peak_rss_kib\":{},\"voluntary_switches\":{}}}",
            frame,
            self.started.elapsed().as_secs_f64(),
            sample.total.as_secs_f64() * 1000.0,
            sample.gpu.as_secs_f64() * 1000.0,
            sample.gpu_queried,
            render.as_secs_f64() * 1000.0,
            update.prepare.as_secs_f64() * 1000.0,
            update.usb.as_secs_f64() * 1000.0,
            update.bytes,
            update.regions,
            cpu_seconds(&own) - self.self_start,
            cpu_seconds(&children) - self.child_start,
            kib("VmRSS:")?,
            kib("VmHWM:")?,
            children.ru_maxrss,
            own.ru_nvcsw
        )?;
        self.writer.flush()
    }
}
