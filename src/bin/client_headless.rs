#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use jpeg_encoder::{ColorType, Encoder};
use remote_control::config::{CLIENT_SERVER_ADDR, TARGET_FPS};
use remote_control::protocol::{ClientHello, write_frame, write_hello};
use scrap::{Capturer, Display};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::HKEY_CURRENT_USER;

const JPEG_QUALITY: u8 = 45;
const SCALE_DIVISOR: usize = 2;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

fn main() {
    let _ = set_autostart_enabled();
    let stop = Arc::new(AtomicBool::new(false));

    while !stop.load(Ordering::Relaxed) {
        if run_stream_session(&stop).is_err() {
            thread::sleep(RECONNECT_DELAY);
        }
    }
}

#[cfg(target_os = "windows")]
fn run_stream_session(stop: &Arc<AtomicBool>) -> Result<()> {
    let mut stream = TcpStream::connect(CLIENT_SERVER_ADDR)
        .with_context(|| format!("connect failed: {CLIENT_SERVER_ADDR}"))?;
    stream.set_nodelay(true)?;
    write_hello(
        &mut stream,
        &ClientHello {
            client_name: client_name(),
        },
    )?;

    let display = Display::primary().context("unable to get primary display")?;
    let mut capturer = Capturer::new(display).context("unable to create screen capturer")?;
    let width = capturer.width();
    let height = capturer.height();
    let frame_interval = Duration::from_millis((1000 / TARGET_FPS).max(1));
    let mut rgb_buf = vec![0_u8; width * height * 3 / (SCALE_DIVISOR * SCALE_DIVISOR)];
    let mut jpeg_buf = Vec::with_capacity(width * height / 8);

    while !stop.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let frame = loop {
            match capturer.frame() {
                Ok(frame) => break frame,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(e).context("capture frame failed"),
            }
        };

        encode_jpeg_bgra_reuse(&frame, width, height, &mut rgb_buf, &mut jpeg_buf)?;
        write_frame(&mut stream, &jpeg_buf).context("send frame failed")?;

        let elapsed = tick.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }

    Ok(())
}

fn encode_jpeg_bgra_reuse(
    frame: &[u8],
    width: usize,
    height: usize,
    rgb_buf: &mut Vec<u8>,
    jpeg_buf: &mut Vec<u8>,
) -> Result<()> {
    let stride = frame.len() / height;
    if stride < width * 4 {
        bail!("unexpected frame stride");
    }

    let encoded_width = width.div_ceil(SCALE_DIVISOR);
    let encoded_height = height.div_ceil(SCALE_DIVISOR);
    rgb_buf.clear();
    rgb_buf.reserve(encoded_width * encoded_height * 3);
    for y in (0..height).step_by(SCALE_DIVISOR) {
        let row = &frame[y * stride..(y * stride + width * 4)];
        for x in (0..width).step_by(SCALE_DIVISOR) {
            let offset = x * 4;
            rgb_buf.extend_from_slice(&[row[offset + 2], row[offset + 1], row[offset]]);
        }
    }

    jpeg_buf.clear();
    let encoder = Encoder::new(jpeg_buf, JPEG_QUALITY);
    encoder.encode(
        rgb_buf,
        encoded_width as u16,
        encoded_height as u16,
        ColorType::Rgb,
    )?;
    Ok(())
}

fn client_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown-user".to_string());
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string());
    format!("{user}@{host}")
}

#[cfg(target_os = "windows")]
fn set_autostart_enabled() -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    let exe = std::env::current_exe().context("current_exe failed")?;
    run_key.set_value(
        "RemoteMonitorClientHeadless",
        &format!("\"{}\"", exe.to_string_lossy()),
    )?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_autostart_enabled() -> Result<()> {
    Ok(())
}
