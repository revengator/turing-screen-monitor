// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(not(target_os = "linux"))]
compile_error!("This application supports Linux only.");

mod levels;
mod metrics;
mod power;
mod profile;
mod protocol;
mod recovery;
mod render;
mod service;

use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const HELP: &str = "turing-screen-monitor (Linux / Turing Rev A 3.5-inch)\n\
  No arguments: sample metrics and save .work/preview.png; no USB access.\n\
  --demo           Use fixed example metrics for a PNG preview\n\
  --preview PATH   Save one PNG without accessing the screen\n\
  --device PATH    Send metrics to a serial device, or 'auto' to detect it\n\
  --once           Send a single frame and exit\n\
  --frames N       Stop after N frames (requires --device)\n\
  --profile PATH   Write new JSONL timing/CPU/RSS measurements (requires --device)\n\
  --full-width-updates  Benchmark legacy full-width row transfers\n\
  --interval SEC   Refresh interval, 0.5..3600 (default: 2)\n\
  --brightness N   Screen brightness, 0..100 (default: 20)\n\
  --reverse        Reverse landscape orientation\n\
  --verbose        Log every successful frame (default: connection events only)\n\
  --help           Show this help\n\
  --version        Show the application version without sampling or USB access\n\
  service --help   Show optional system service installation commands\n\
Continuous monitoring retries USB failures. --once, --frames and --profile fail immediately.\n\
Ctrl+C or SIGTERM stops monitoring and leaves the last frame on screen.";

struct Options {
    demo: bool,
    preview: Option<PathBuf>,
    device: Option<String>,
    interval: Duration,
    brightness: u8,
    reverse: bool,
    once: bool,
    frames: Option<usize>,
    profile: Option<PathBuf>,
    full_width_updates: bool,
    verbose: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut result = Self {
            demo: false,
            preview: None,
            device: None,
            interval: Duration::from_secs(2),
            brightness: 20,
            reverse: false,
            once: false,
            frames: None,
            profile: None,
            full_width_updates: false,
            verbose: false,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--preview" => {
                    result.preview = Some(args.next().ok_or("--preview needs a path")?.into())
                }
                "--device" => {
                    result.device = Some(args.next().ok_or("--device needs a path or auto")?)
                }
                "--interval" => {
                    let seconds = args
                        .next()
                        .ok_or("--interval needs seconds")?
                        .parse::<f64>()?;
                    if !seconds.is_finite() || !(0.5..=3600.0).contains(&seconds) {
                        return Err("Interval must be 0.5..3600 seconds".into());
                    }
                    result.interval = Duration::from_secs_f64(seconds);
                }
                "--brightness" => {
                    result.brightness =
                        args.next().ok_or("--brightness needs a number")?.parse()?;
                    if result.brightness > 100 {
                        return Err("Brightness must be 0..100".into());
                    }
                }
                "--demo" => result.demo = true,
                "--reverse" => result.reverse = true,
                "--verbose" => result.verbose = true,
                "--once" => result.once = true,
                "--full-width-updates" => result.full_width_updates = true,
                "--frames" => {
                    let frames = args.next().ok_or("--frames needs a count")?.parse()?;
                    if frames == 0 {
                        return Err("Frame count must be positive".into());
                    }
                    result.frames = Some(frames);
                }
                "--profile" => {
                    result.profile = Some(args.next().ok_or("--profile needs a path")?.into())
                }
                _ => return Err(format!("Unknown argument: {arg}; use --help").into()),
            }
        }
        if result.demo && result.device.is_some() {
            return Err("--demo is only available for PNG previews".into());
        }
        if result.device.is_some() && result.preview.is_some() {
            return Err("Use either --preview or --device".into());
        }
        if result.device.is_none()
            && (result.profile.is_some() || result.frames.is_some() || result.full_width_updates)
        {
            return Err("--frames, --profile and --full-width-updates require --device".into());
        }
        if result.once && result.frames.is_some() {
            return Err("Use either --once or --frames".into());
        }
        if result.device.is_none() && result.preview.is_none() {
            result.preview = Some(".work/preview.png".into());
        }
        Ok(result)
    }
}

// Wake promptly for termination; keep idle suspend checks no more than a second apart.
fn wait(
    duration: Duration,
    running: &AtomicBool,
    resume: &mut recovery::ResumeDetector,
) -> std::io::Result<bool> {
    let started = Instant::now();
    while running.load(Ordering::Relaxed) && started.elapsed() < duration {
        thread::park_timeout(
            Duration::from_secs(1).min(duration.saturating_sub(started.elapsed())),
        );
        if resume.resumed()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn connect(options: &Options) -> Result<protocol::Display, Box<dyn std::error::Error>> {
    let requested = options.device.as_deref().unwrap();
    // Resolve again on each attempt: ttyACM numbers can change after re-enumeration.
    let path = if requested == "auto" {
        protocol::detect()?
    } else {
        requested.into()
    };
    let display = protocol::Display::open(
        &path.to_string_lossy(),
        options.brightness,
        options.reverse,
        options.full_width_updates,
    )?;
    eprintln!("Connected to {}. Sending a full frame.", path.display());
    Ok(display)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if env::args().nth(1).as_deref() == Some("service") {
        return service::run(env::args().skip(2));
    }
    if env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if env::args().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let options = Options::parse(env::args().skip(1))?;
    let mut profile = options
        .profile
        .as_deref()
        .map(profile::Profile::new)
        .transpose()?;
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let main_thread = thread::current();
    ctrlc::set_handler(move || {
        flag.store(false, Ordering::Relaxed);
        main_thread.unpark();
    })?;
    let mut resume = recovery::ResumeDetector::new()?;
    let mut metrics = if options.demo {
        metrics::Metrics::demo()
    } else {
        let mut live = metrics::Metrics::new()?;
        // A first delta is needed for a meaningful CPU percentage.
        thread::sleep(Duration::from_millis(500));
        live.sample()?;
        live
    };
    if !running.load(Ordering::Relaxed) {
        return Ok(());
    }
    eprintln!("CPU: {}", metrics.cpu_name);
    match metrics.cpu_power_watts {
        Some(watts) => eprintln!("CPU package power: {watts:.1} W"),
        None => eprintln!("CPU package power: unavailable; field hidden"),
    }
    eprintln!("GPU: {}", metrics.gpu_status);
    for disk in &metrics.disks {
        match disk.space {
            Some(s) => eprintln!(
                "Disk {}: {:.0}% | {:.3}/{:.3} GiB",
                disk.mount, s.percent, s.used_gib, s.total_gib
            ),
            None => eprintln!(
                "Disk {}: {}",
                disk.mount,
                if disk.mounted {
                    "unreadable"
                } else {
                    "not mounted"
                }
            ),
        }
    }
    if let Some(path) = options.preview.as_ref() {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        render::render(&metrics).save(path)?;
        println!("Preview: {} (480x320; no USB access)", path.display());
        return Ok(());
    }
    let retry = !options.once && options.frames.is_none() && options.profile.is_none();
    let mut backoff = recovery::Backoff::default();
    let mut display = None;
    let mut resumed = false;
    let mut frames = 0;
    while running.load(Ordering::Relaxed) {
        if std::mem::take(&mut resumed) || resume.resumed()? {
            eprintln!("System resumed; resetting sensor baselines and USB connection.");
            display = None;
            metrics.reset_after_resume()?;
            backoff.reset();
        }
        if display.is_none() {
            match connect(&options) {
                Ok(connection) => display = Some(connection),
                Err(error) => {
                    if !retry || error.is::<protocol::UnsupportedModel>() {
                        return Err(error);
                    }
                    let delay = backoff.next_delay();
                    eprintln!(
                        "USB unavailable: {error}. Retrying in {} s.",
                        delay.as_secs()
                    );
                    resumed = wait(delay, &running, &mut resume)?;
                    continue;
                }
            }
        }
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let sample_timing = metrics.sample()?;
        // A suspend during initialization or sampling must not publish stale readings.
        if resume.resumed()? {
            resumed = true;
            continue;
        }
        let started = Instant::now();
        let frame = render::render(&metrics);
        let render_time = started.elapsed();
        let update = match display.as_mut().unwrap().update(frame) {
            Ok(update) => update,
            Err(error) => {
                if !retry {
                    return Err(error.into());
                }
                // Discard both the descriptor and cached image after any incomplete transfer.
                display = None;
                let delay = backoff.next_delay();
                eprintln!(
                    "USB transfer failed: {error}. Reconnecting in {} s.",
                    delay.as_secs()
                );
                resumed = wait(delay, &running, &mut resume)?;
                continue;
            }
        };
        backoff.reset();
        if options.verbose || !retry {
            eprintln!(
                "CPU {} | GPU {} | sent {} bytes in {} ms",
                render::number(metrics.cpu_usage, "%"),
                metrics.gpu_status,
                update.bytes,
                started.elapsed().as_millis()
            );
        }
        frames += 1;
        if let Some(profile) = profile.as_mut() {
            profile.record(frames, &sample_timing, render_time, &update)?;
        }
        if options.once || options.frames.is_some_and(|limit| frames >= limit) {
            break;
        }
        resumed = wait(
            options.interval.saturating_sub(started.elapsed()),
            &running,
            &mut resume,
        )?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_options_fail_before_io() {
        for args in [
            vec!["--demo", "--device", "auto"],
            vec!["--interval", "NaN"],
            vec!["--interval", "0"],
            vec!["--brightness", "101"],
            vec!["--device"],
            vec!["--device", "auto", "--preview", "a.png"],
            vec!["--device", "auto", "--frames", "0"],
            vec!["--frames", "2"],
            vec!["--profile", "a.jsonl"],
            vec!["--full-width-updates"],
            vec!["--device", "auto", "--once", "--frames", "2"],
        ] {
            assert!(Options::parse(args.into_iter().map(String::from)).is_err());
        }
    }
    #[test]
    fn defaults_never_open_usb() {
        let options = Options::parse(Vec::new()).unwrap();
        assert!(options.device.is_none());
        assert_eq!(options.preview, Some(PathBuf::from(".work/preview.png")));
    }
}
