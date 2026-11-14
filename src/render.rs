// SPDX-License-Identifier: GPL-3.0-or-later
use crate::levels::{self, Level, Thresholds};
use crate::metrics::Metrics;
use font8x8::UnicodeFonts;
use std::{
    fs::File,
    io::{self, BufWriter},
    path::Path,
    sync::OnceLock,
};

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 320;
pub type Color = [u8; 3];
const BG: Color = [5, 14, 24];
const PANEL: Color = [10, 27, 42];
const BORDER: Color = [27, 89, 112];
const TRACK: Color = [26, 57, 76];
const TEXT: Color = [222, 236, 244];
const MUTED: Color = [133, 162, 180];

pub struct Frame {
    pub rgb: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

struct RingPixel {
    dx: isize,
    dy: isize,
    fraction: f64,
}

// Calculate ring geometry once; rendering subsequent frames only selects colors.
fn ring_pixels(radius: usize) -> &'static [RingPixel] {
    static RINGS: OnceLock<[Vec<RingPixel>; 3]> = OnceLock::new();
    let rings = RINGS.get_or_init(|| {
        [44_isize, 40, 32].map(|r| {
            let mut pixels = Vec::new();
            let inner = r - if r == 32 { 5 } else { 6 };
            for dy in -r..=r {
                for dx in -r..=r {
                    let distance = dx * dx + dy * dy;
                    if distance >= inner * inner && distance <= r * r {
                        let fraction = ((dy as f64).atan2(dx as f64) + std::f64::consts::FRAC_PI_2)
                            .rem_euclid(std::f64::consts::TAU)
                            / std::f64::consts::TAU;
                        pixels.push(RingPixel { dx, dy, fraction });
                    }
                }
            }
            pixels
        })
    });
    &rings[match radius {
        44 => 0,
        40 => 1,
        32 => 2,
        _ => unreachable!("Unsupported ring radius"),
    }]
}

impl Frame {
    pub fn new() -> Self {
        Self {
            rgb: BG.repeat(WIDTH * HEIGHT),
        }
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        for row in y..(y + h).min(HEIGHT) {
            for col in x..(x + w).min(WIDTH) {
                self.rgb[(row * WIDTH + col) * 3..(row * WIDTH + col + 1) * 3]
                    .copy_from_slice(&color);
            }
        }
    }

    fn text(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        scale: usize,
        color: Color,
        max_chars: usize,
    ) {
        for (i, c) in text.chars().take(max_chars).enumerate() {
            let Some(glyph) = font8x8::BASIC_FONTS.get(c) else {
                continue;
            };
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        self.rect(
                            x + i * 8 * scale + col * scale,
                            y + row * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }

    fn panel(&mut self, x: usize, y: usize, w: usize, h: usize, accent: Color) {
        // Rounded corners with a one-pixel border, all inside the panel bounds.
        const OFFSETS: [usize; 8] = [6, 4, 3, 2, 1, 1, 0, 0];
        for row in 0..h {
            let edge = row.min(h - row - 1);
            let inset = OFFSETS.get(edge).copied().unwrap_or(0);
            self.rect(x + inset, y + row, w - 2 * inset, 1, accent.map(|c| c / 2));
            if row > 0 && row < h - 1 {
                self.rect(x + inset + 1, y + row, w - 2 * inset - 2, 1, PANEL);
            }
        }
    }

    fn ring(&mut self, cx: usize, cy: usize, radius: usize, value: Option<f64>, accent: Color) {
        let fraction = value
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 100.0) / 100.0);
        for pixel in ring_pixels(radius) {
            let x = cx.checked_add_signed(pixel.dx);
            let y = cy.checked_add_signed(pixel.dy);
            if let (Some(x), Some(y)) = (x, y)
                && x < WIDTH
                && y < HEIGHT
            {
                let color = if fraction.is_some_and(|f| pixel.fraction < f) {
                    accent
                } else {
                    TRACK
                };
                let index = (y * WIDTH + x) * 3;
                self.rgb[index..index + 3].copy_from_slice(&color);
            }
        }
    }

    fn processor_header(&mut self, x: usize, title: &str, model: &str, level: Level) {
        self.panel(x, 8, 228, 164, level.color());
        self.text(
            x + 12,
            20,
            &format!("{title} {}", level.label()),
            1,
            level.color(),
            26,
        );
        if model.chars().count() <= 13 {
            self.text(x + 12, 38, model, 2, TEXT, 13);
        } else {
            let characters: Vec<char> = model.chars().collect();
            for (index, line) in characters.chunks(26).take(2).enumerate() {
                self.text(
                    x + 12,
                    36 + index * 11,
                    &line.iter().collect::<String>(),
                    1,
                    TEXT,
                    26,
                );
            }
        }
    }

    fn processor(
        &mut self,
        x: usize,
        title: &str,
        model: &str,
        usage: Option<f64>,
        temperature: Option<f64>,
    ) {
        let load_level = Level::classify(usage, levels::LOAD);
        let temperature_level = Level::classify(temperature, levels::CPU_TEMPERATURE);
        self.processor_header(x, title, model, load_level.max(temperature_level));
        let usage = usage.filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
        let value = number(usage, "");
        let value_x = x + 62 - value.len() * 12;
        self.ring(x + 62, 116, 44, usage, load_level.color());
        self.text(value_x, 104, &value, 3, load_level.color(), 3);
        self.text(x + 38, 139, "LOAD %", 1, MUTED, 6);
        self.text(
            x + 62 - load_level.label().len() * 4,
            162,
            load_level.label(),
            1,
            load_level.color(),
            6,
        );
        self.text(x + 124, 78, "TEMP", 1, MUTED, 10);
        self.text(
            x + 124,
            94,
            &format!("{} C", number(temperature, "")),
            2,
            temperature_level.color(),
            6,
        );
    }

    fn gpu_gauge(&mut self, cx: usize, title: &str, value: Option<f64>, thresholds: Thresholds) {
        let level = Level::classify(value, thresholds);
        let value = value.filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
        self.text(cx - title.len() * 4, 63, title, 1, MUTED, 10);
        self.ring(cx, 111, 40, value, level.color());
        let text = number(value, "%");
        self.text(cx - text.len() * 8, 103, &text, 2, level.color(), 4);
        self.text(
            cx - level.label().len() * 4,
            126,
            level.label(),
            1,
            level.color(),
            6,
        );
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let writer = BufWriter::new(File::create(path)?);
        let mut encoder = png::Encoder::new(writer, WIDTH as u32, HEIGHT as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.rgb)?;
        writer.finish()?;
        Ok(())
    }

    pub fn rgb565_region(&self, region: Region) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(region.width * region.height * 2);
        for y in region.y..region.y + region.height {
            let start = (y * WIDTH + region.x) * 3;
            for pixel in self.rgb[start..start + region.width * 3].chunks_exact(3) {
                let value = ((pixel[0] as u16 >> 3) << 11)
                    | ((pixel[1] as u16 >> 2) << 5)
                    | (pixel[2] as u16 >> 3);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    // Bound consecutive changed rows horizontally as well as vertically.
    pub fn changed_regions(&self, previous: Option<&Frame>) -> Vec<Region> {
        let Some(previous) = previous else {
            return vec![Region {
                x: 0,
                y: 0,
                width: WIDTH,
                height: HEIGHT,
            }];
        };
        let mut regions = Vec::new();
        let mut active: Option<Region> = None;
        for row in 0..HEIGHT {
            let range = row * WIDTH * 3..(row + 1) * WIDTH * 3;
            let current = &self.rgb[range.clone()];
            let before = &previous.rgb[range];
            if current != before {
                let first = current
                    .chunks_exact(3)
                    .zip(before.chunks_exact(3))
                    .position(|(a, b)| a != b)
                    .unwrap();
                let last = current
                    .chunks_exact(3)
                    .zip(before.chunks_exact(3))
                    .rposition(|(a, b)| a != b)
                    .unwrap();
                if let Some(region) = active.as_mut() {
                    let right = (region.x + region.width).max(last + 1);
                    region.x = region.x.min(first);
                    region.width = right - region.x;
                    region.height += 1;
                } else {
                    active = Some(Region {
                        x: first,
                        y: row,
                        width: last + 1 - first,
                        height: 1,
                    });
                }
            } else if let Some(region) = active.take() {
                regions.push(region);
            }
        }
        if let Some(region) = active {
            regions.push(region);
        }
        regions
    }
}

pub fn number(value: Option<f64>, suffix: &str) -> String {
    value
        .filter(|v| v.is_finite())
        .map(|v| format!("{v:.0}{suffix}"))
        .unwrap_or_else(|| "N/A".into())
}

fn hardware_model(name: &str) -> String {
    let clean = name.replace("(R)", "").replace("(TM)", "");
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    for prefix in ["Intel Core ", "NVIDIA GeForce ", "NVIDIA ", "AMD "] {
        if let Some(model) = clean.strip_prefix(prefix) {
            return model.to_string();
        }
    }
    clean
}

fn disk_amount(gib: f64) -> (String, &'static str) {
    if gib >= 1024.0 {
        (format!("{:.2}", gib / 1024.0), "TiB")
    } else if gib > 0.0 && gib < 0.1 {
        (format!("{:.1}", gib * 1024.0), "MiB")
    } else {
        (format!("{gib:.1}"), "GiB")
    }
}

fn disk_capacity(disk: &crate::metrics::Disk) -> String {
    disk.space
        .map(|s| {
            let (used, used_unit) = disk_amount(s.used_gib);
            let (total, total_unit) = disk_amount(s.total_gib);
            if used_unit == total_unit {
                format!("{used}/{total}{total_unit}")
            } else {
                format!("{used}{used_unit}/{total}{total_unit}")
            }
        })
        .unwrap_or_else(|| {
            if disk.mounted {
                "N/A - READ ERROR"
            } else {
                "NOT MOUNTED"
            }
            .into()
        })
}

pub fn render(m: &Metrics) -> Frame {
    let mut f = Frame::new();
    f.processor(
        8,
        "CPU",
        &hardware_model(&m.cpu_name),
        m.cpu_usage,
        m.cpu_temperature,
    );
    if let Some(watts) = m.cpu_power_watts.filter(|v| v.is_finite() && *v >= 0.0) {
        let power = format!("POWER {watts:.1} W");
        f.text(224 - power.len().min(15) * 8, 20, &power, 1, MUTED, 15);
    }
    f.text(132, 125, "FREQ. GHz", 1, MUTED, 11);
    let frequency = m
        .cpu_ghz
        .filter(|v| v.is_finite())
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "N/A".into());
    f.text(132, 141, &frequency, 2, TEXT, 5);

    let gpu = m.gpu.as_ref();
    let gpu_name = gpu
        .map(|g| hardware_model(&g.name))
        .unwrap_or_else(|| "NO DATA".into());
    let gpu_load = Level::classify(gpu.and_then(|g| g.usage), levels::LOAD);
    let vram_level = Level::classify(gpu.and_then(|g| g.memory_percentage()), levels::MEMORY);
    let gpu_temperature = Level::classify(gpu.and_then(|g| g.temperature), levels::GPU_TEMPERATURE);
    f.processor_header(
        244,
        "GPU",
        &gpu_name,
        gpu_load.max(vram_level).max(gpu_temperature),
    );
    let power = gpu
        .and_then(|g| g.power_watts)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| format!("POWER {v:.1} W"))
        .unwrap_or_else(|| "POWER N/A W".into());
    f.text(460 - power.len().min(15) * 8, 20, &power, 1, MUTED, 15);
    f.gpu_gauge(300, "LOAD", gpu.and_then(|g| g.usage), levels::LOAD);
    f.gpu_gauge(
        412,
        "VRAM",
        gpu.and_then(|g| g.memory_percentage()),
        levels::MEMORY,
    );
    f.text(
        256,
        158,
        &format!("TEMP {} C", number(gpu.and_then(|g| g.temperature), "")),
        1,
        gpu_temperature.color(),
        13,
    );
    let vram = gpu
        .and_then(|g| Some((g.used_mib?, g.total_mib?)))
        .map(|(used, total)| format!("{:.1}/{:.1} GiB", used / 1024.0, total / 1024.0))
        .unwrap_or_else(|| "N/A".into());
    f.text(364, 158, &vram, 1, vram_level.color(), 12);

    let ram_percent = m
        .memory
        .filter(|(_, total)| *total > 0.0)
        .map(|(used, total)| used / total * 100.0);
    let ram_level = Level::classify(ram_percent, levels::MEMORY);
    f.panel(8, 180, 228, 110, ram_level.color());
    f.text(
        20,
        192,
        &format!("RAM {}", ram_level.label()),
        1,
        ram_level.color(),
        12,
    );
    let value = number(ram_percent, "");
    f.ring(54, 239, 32, ram_percent, ram_level.color());
    f.text(54 - value.len() * 8, 231, &value, 2, ram_level.color(), 3);
    f.text(50, 252, "%", 1, MUTED, 1);
    let used = m
        .memory
        .map(|(used, _)| format!("{used:.1}"))
        .unwrap_or_else(|| "N/A".into());
    let total = m
        .memory
        .map(|(_, total)| format!("{total:.1} GiB"))
        .unwrap_or_else(|| "N/A".into());
    f.text(144, 192, &total, 1, MUTED, 10);
    f.text(112, 222, &used, 2, ram_level.color(), 5);
    f.text(112, 247, "GiB used", 1, MUTED, 10);

    let root = m.disks.iter().find(|disk| disk.mount == "/");
    let space = root.filter(|disk| disk.mounted).and_then(|disk| disk.space);
    let percent = space.map(|s| s.percent);
    let disk_level = Level::classify(percent, levels::DISK);
    f.panel(244, 180, 228, 110, disk_level.color());
    f.text(
        256,
        192,
        &format!("DISK {}", disk_level.label()),
        1,
        disk_level.color(),
        12,
    );
    let value = number(percent, "");
    f.ring(286, 239, 32, percent, disk_level.color());
    f.text(286 - value.len() * 8, 231, &value, 2, disk_level.color(), 3);
    f.text(282, 252, "%", 1, MUTED, 1);
    f.text(330, 215, "/", 2, TEXT, 1);
    let capacity = root.map(disk_capacity).unwrap_or_else(|| "N/A".into());
    f.text(330, 240, &capacity, 1, TEXT, 16);
    f.text(330, 258, "used/total", 1, MUTED, 12);

    f.rect(12, 298, 456, 1, BORDER);
    let uptime = m
        .uptime
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| {
            let minutes = s as u64 / 60;
            format!(
                "UPTIME {}d {:02}h {:02}m",
                minutes / 1440,
                minutes / 60 % 24,
                minutes % 60
            )
        })
        .unwrap_or_else(|| "UPTIME N/A".into());
    f.text(12, 307, &uptime, 1, MUTED, 39);
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    const TEST_COLOR: Color = [42, 220, 232];

    #[test]
    fn cpu_power_is_hidden_without_valid_data_and_zero_is_a_real_reading() {
        let mut data = Metrics::demo();
        let hidden = render(&data);
        for value in [None, Some(f64::NAN), Some(f64::INFINITY), Some(-1.0)] {
            data.cpu_power_watts = value;
            assert_eq!(render(&data).rgb, hidden.rgb);
        }
        data.cpu_power_watts = Some(0.0);
        assert_ne!(render(&data).rgb, hidden.rgb);
        data.cpu_power_watts = Some(24.5);
        let visible = render(&data);
        assert_ne!(visible.rgb, hidden.rgb);
        if let Some(directory) = std::env::var_os("TURING_TEST_PREVIEW_DIR") {
            let directory = Path::new(&directory);
            std::fs::create_dir_all(directory).unwrap();
            visible.save(&directory.join("cpu-power.png")).unwrap();
        }
    }

    #[test]
    fn rendered_levels_distinguish_load_temperature_and_missing_data() {
        let mut data = Metrics::demo();
        data.cpu_usage = Some(0.0);
        let low = render(&data);
        let has_color =
            |frame: &Frame, level: Level| frame.rgb.chunks_exact(3).any(|p| p == level.color());
        assert!(has_color(&low, Level::Low)); // Zero load still has a visible level.
        assert!(!has_color(&low, Level::High));
        assert!(!has_color(&low, Level::Medium));
        data.cpu_temperature = Some(85.0); // Low load must not hide high temperature.
        data.gpu.as_mut().unwrap().usage = Some(60.0);
        data.gpu.as_mut().unwrap().used_mib = Some(15_000.0);
        data.memory = Some((24.0, 32.0));
        let mixed = render(&data);
        for level in [Level::Low, Level::Medium, Level::High] {
            assert!(has_color(&mixed, level));
        }
        data.cpu_usage = None;
        data.cpu_temperature = None;
        data.gpu = None;
        data.memory = None;
        for disk in &mut data.disks {
            disk.mounted = false;
            disk.space = None;
        }
        let missing = render(&data);
        assert!(has_color(&missing, Level::Unavailable));
        for level in [Level::Low, Level::Medium, Level::High] {
            assert!(!has_color(&missing, level));
        }
        // Optional offline design samples. Never send these synthetic values to hardware.
        if let Some(directory) = std::env::var_os("TURING_TEST_PREVIEW_DIR") {
            let directory = Path::new(&directory);
            std::fs::create_dir_all(directory).unwrap();
            for (name, frame) in [
                ("low.png", low),
                ("mixed.png", mixed),
                ("missing.png", missing),
            ] {
                frame.save(&directory.join(name)).unwrap();
            }
        }
    }
    #[test]
    fn partial_updates_bound_changed_pixels_including_last_row() {
        let previous = Frame::new();
        let mut next = Frame::new();
        assert!(next.changed_regions(Some(&previous)).is_empty());
        next.rect(5, 6, 2, 3, TEST_COLOR);
        next.rect(0, HEIGHT - 1, 1, 1, TEXT);
        assert_eq!(
            next.changed_regions(Some(&previous)),
            vec![
                Region {
                    x: 5,
                    y: 6,
                    width: 2,
                    height: 3
                },
                Region {
                    x: 0,
                    y: 319,
                    width: 1,
                    height: 1
                }
            ]
        );
        assert_eq!(
            next.changed_regions(None),
            vec![Region {
                x: 0,
                y: 0,
                width: WIDTH,
                height: HEIGHT
            }]
        );
    }
    #[test]
    fn rgb565_has_correct_channel_order_and_endianness() {
        let mut frame = Frame::new();
        frame.rect(0, 0, 1, 1, [255, 0, 0]);
        frame.rect(1, 0, 1, 1, [0, 255, 0]);
        frame.rect(2, 0, 1, 1, [0, 0, 255]);
        assert_eq!(
            frame.rgb565_region(Region {
                x: 0,
                y: 0,
                width: 3,
                height: 1
            }),
            [0, 248, 224, 7, 31, 0]
        );
    }

    #[test]
    fn applying_partial_payloads_reconstructs_full_rgb565_image() {
        let previous = Frame::new();
        let mut next = Frame::new();
        for (x, y, w, h, color) in [
            (0, 0, 1, 1, TEST_COLOR),
            (12, 20, 8, 2, TEXT),
            (400, 21, 5, 4, TEST_COLOR),
            (479, 319, 1, 1, TEXT),
        ] {
            next.rect(x, y, w, h, color);
        }
        let full = Region {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
        };
        let mut reconstructed = previous.rgb565_region(full);
        let mut transmitted = 0;
        for region in next.changed_regions(Some(&previous)) {
            let payload = next.rgb565_region(region);
            transmitted += payload.len();
            for row in 0..region.height {
                let start = ((region.y + row) * WIDTH + region.x) * 2;
                reconstructed[start..start + region.width * 2].copy_from_slice(
                    &payload[row * region.width * 2..(row + 1) * region.width * 2],
                );
            }
        }
        assert_eq!(reconstructed, next.rgb565_region(full));
        assert!(transmitted < WIDTH * HEIGHT * 2 / 10);
    }
}
