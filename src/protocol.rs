// SPDX-License-Identifier: GPL-3.0-or-later
use crate::render::{Frame, HEIGHT, WIDTH};
use std::{
    fmt,
    io::{self, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct UnsupportedModel;

impl fmt::Display for UnsupportedModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Unexpected model response; only Rev A 3.5-inch displays are supported")
    }
}

impl std::error::Error for UnsupportedModel {}

#[repr(u8)]
#[derive(Clone, Copy)]
enum Opcode {
    ScreenOn = 109,
    Brightness = 110,
    Bitmap = 197,
}

struct Packet([u8; 6]);

impl Packet {
    fn encode(opcode: Opcode, coordinates: [u16; 4]) -> Self {
        let packed = coordinates.into_iter().fold(0_u64, |bits, coordinate| {
            assert!(coordinate < 1024);
            (bits << 10) | u64::from(coordinate)
        });
        let encoded = packed.to_be_bytes();
        Self([
            encoded[3],
            encoded[4],
            encoded[5],
            encoded[6],
            encoded[7],
            opcode as u8,
        ])
    }

    fn light(percent: u8) -> Self {
        let setting = (255.0 - f64::from(percent) / 100.0 * 255.0).trunc() as u16;
        Self::encode(Opcode::Brightness, [setting, 0, 0, 0])
    }
}

fn landscape_packet(reversed: bool) -> [u8; 16] {
    let mut packet = [0_u8; 16];
    packet[5..11].copy_from_slice(&[
        121,
        if reversed { 103 } else { 102 },
        (WIDTH >> 8) as u8,
        WIDTH as u8,
        (HEIGHT >> 8) as u8,
        HEIGHT as u8,
    ]);
    packet
}

fn identify(stream: &mut (impl Read + Write)) -> Result<(), Box<dyn std::error::Error>> {
    stream.write_all(&[0x45; 6])?;
    let mut response = Vec::with_capacity(6);
    while response.len() < 6 {
        let mut buffer = [0_u8; 6];
        let remaining = 6 - response.len();
        match stream.read(&mut buffer[..remaining]) {
            Ok(0) => break,
            Ok(count) => response.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => break,
            Err(error) => return Err(error.into()),
        }
    }
    if response.is_empty() || response == [1; 6] {
        Ok(())
    } else {
        Err(UnsupportedModel.into())
    }
}

pub fn detect() -> io::Result<PathBuf> {
    let candidates: Vec<_> = std::fs::read_dir("/dev/serial/by-id")?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("USB35INCHIPSV2")
        })
        .map(|entry| entry.path())
        .collect();
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Turing USB35INCHIPSV2 not found; use --device with an explicit path",
        )),
        _ => Err(io::Error::other(
            "Multiple Turing screens found; select --device explicitly",
        )),
    }
}

struct SerialTransport(Box<dyn serialport::SerialPort>);

impl SerialTransport {
    fn connect(
        path: &str,
        brightness: u8,
        reversed: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if brightness > 100 {
            return Err("Brightness must be 0..100".into());
        }
        let mut connection = Self(
            serialport::new(path, 115_200)
                .data_bits(serialport::DataBits::Eight)
                .parity(serialport::Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::Hardware)
                .timeout(Duration::from_secs(2))
                .open()?,
        );
        connection.0.clear(serialport::ClearBuffer::Input)?;
        identify(&mut connection.0)?;
        connection.0.write_all(&landscape_packet(reversed))?;
        connection.0.write_all(&Packet::light(brightness).0)?;
        connection
            .0
            .write_all(&Packet::encode(Opcode::ScreenOn, [0; 4]).0)?;
        Ok(connection)
    }

    fn rectangle(&mut self, packet: Packet, pixels: &[u8]) -> io::Result<()> {
        self.0.write_all(&packet.0)?;
        for payload in pixels.chunks(3840) {
            self.0.write_all(payload)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct UpdateStats {
    pub bytes: usize,
    pub regions: usize,
    pub prepare: Duration,
    pub usb: Duration,
}

pub struct Display {
    transport: SerialTransport,
    last_frame: Option<Frame>,
    expand_rows: bool,
}

impl Display {
    pub fn open(
        path: &str,
        level: u8,
        reverse: bool,
        full_width_updates: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            transport: SerialTransport::connect(path, level, reverse)?,
            last_frame: None,
            expand_rows: full_width_updates,
        })
    }

    pub fn update(&mut self, frame: Frame) -> io::Result<UpdateStats> {
        let preparation = Instant::now();
        let rectangles = frame.changed_regions(self.last_frame.as_ref());
        let mut report = UpdateStats {
            regions: rectangles.len(),
            prepare: preparation.elapsed(),
            ..Default::default()
        };
        for mut rectangle in rectangles {
            let preparation = Instant::now();
            if self.expand_rows {
                rectangle.x = 0;
                rectangle.width = WIDTH;
            }
            let packet = Packet::encode(
                Opcode::Bitmap,
                [
                    rectangle.x as u16,
                    rectangle.y as u16,
                    (rectangle.x + rectangle.width - 1) as u16,
                    (rectangle.y + rectangle.height - 1) as u16,
                ],
            );
            let payload = frame.rgb565_region(rectangle);
            report.prepare += preparation.elapsed();
            let transfer = Instant::now();
            self.transport.rectangle(packet, &payload)?;
            report.usb += transfer.elapsed();
            report.bytes += 6 + payload.len();
        }
        let transfer = Instant::now();
        self.transport.0.flush()?;
        report.usb += transfer.elapsed();
        self.last_frame = Some(frame);
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brightness_matches_expected_wire_packets() {
        assert_eq!(Packet::light(25).0, [0x2f, 0xc0, 0, 0, 0, 0x6e]);
        assert_eq!(Packet::light(0).0, [0x3f, 0xc0, 0, 0, 0, 0x6e]);
        assert_eq!(Packet::light(100).0, [0, 0, 0, 0, 0, 0x6e]);
    }
    #[test]
    fn landscape_and_bitmap_bounds_match_native_dimensions() {
        assert_eq!(
            landscape_packet(false),
            [0, 0, 0, 0, 0, 121, 102, 1, 224, 1, 64, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            Packet::encode(Opcode::Bitmap, [0, 0, 479, 319]).0,
            [0, 0, 7, 125, 63, 197]
        );
        // Landscape rectangle: x=10, y=20, width=100, height=200.
        assert_eq!(
            Packet::encode(Opcode::Bitmap, [10, 20, 109, 219]).0,
            [0x02, 0x81, 0x41, 0xb4, 0xdb, 0xc5]
        );
        assert_eq!(landscape_packet(true)[6], 103);
    }
    #[test]
    fn identification_accumulates_fragments_and_rejects_incomplete_responses() {
        struct Peer {
            fragments: std::collections::VecDeque<Vec<u8>>,
            sent: Vec<u8>,
        }
        impl Read for Peer {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                match self.fragments.pop_front() {
                    Some(fragment) => {
                        buffer[..fragment.len()].copy_from_slice(&fragment);
                        Ok(fragment.len())
                    }
                    None => Err(io::ErrorKind::TimedOut.into()),
                }
            }
        }
        impl Write for Peer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.sent.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for (fragments, supported) in [
            (vec![], true),
            (vec![vec![1; 6]], true),
            (vec![vec![1; 2], vec![1], vec![1; 3]], true),
            (vec![vec![1; 5]], false),
            (vec![vec![2; 6]], false),
            (vec![vec![1; 5], vec![2]], false),
        ] {
            let mut peer = Peer {
                fragments: fragments.into(),
                sent: Vec::new(),
            };
            let result = identify(&mut peer);
            assert_eq!(result.is_ok(), supported);
            if let Err(error) = result {
                assert!(error.is::<UnsupportedModel>());
            }
            assert_eq!(peer.sent, [0x45; 6]);
        }
    }
}
