#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use jpeg_encoder::{ColorType, Encoder};
use remote_control::config::{CLIENT_SERVER_ADDR, TARGET_FPS};
use remote_control::platform::{
    acquire_single_instance, cursor_position, enable_dpi_awareness,
    set_interactive_user_startup_task, startup_task_exists,
};
use remote_control::protocol::{ClientHello, write_frame, write_hello};
use scrap::{Capturer, Display};
use std::io::ErrorKind;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::HKEY_CURRENT_USER;

const JPEG_QUALITY: u8 = 90;
const SCALE_DIVISOR: usize = 2;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const SINGLE_INSTANCE_NAME: &str = "RemoteMonitorClient";
const STARTUP_TASK_NAME: &str = "RemoteMonitorClientSystemStartup";
#[cfg(target_os = "windows")]
const REGISTRY_VALUE_NAME: &str = "RemoteMonitorClientHeadless";

fn main() {
    enable_dpi_awareness();
    log_headless_event("process started");

    let Some(_instance_guard) = acquire_single_instance(SINGLE_INSTANCE_NAME) else {
        log_headless_event("another client instance is already running; exiting");
        return;
    };

    if let Err(e) = set_autostart_enabled() {
        log_headless_event(&format!("set autostart failed: {e:#}"));
    }
    if let Err(e) = set_registry_autostart_enabled() {
        log_headless_event(&format!("set registry autostart failed: {e:#}"));
    }

    let stop = Arc::new(AtomicBool::new(false));

    while !stop.load(Ordering::Relaxed) {
        match run_stream_session(&stop) {
            Ok(()) => log_headless_event("stream session ended"),
            Err(e) => log_headless_event(&format!("stream session failed: {e:#}")),
        }
        if !stop.load(Ordering::Relaxed) {
            thread::sleep(RECONNECT_DELAY);
        }
    }
}

#[cfg(target_os = "windows")]
fn run_stream_session(stop: &Arc<AtomicBool>) -> Result<()> {
    log_headless_event(&format!("connecting to {CLIENT_SERVER_ADDR}"));
    let mut stream = TcpStream::connect(CLIENT_SERVER_ADDR)
        .with_context(|| format!("connect failed: {CLIENT_SERVER_ADDR}"))?;
    stream.set_nodelay(true)?;
    write_hello(
        &mut stream,
        &ClientHello {
            client_name: client_name(),
        },
    )?;
    log_headless_event("connected and hello sent");

    let display = Display::primary().context("unable to get primary display")?;
    let mut capturer = Capturer::new(display).context("unable to create screen capturer")?;
    let width = capturer.width();
    let height = capturer.height();
    let frame_interval = Duration::from_millis((1000 / TARGET_FPS).max(1));
    let encoded_pixels = width.div_ceil(SCALE_DIVISOR) * height.div_ceil(SCALE_DIVISOR);
    let mut rgb_buf = Vec::with_capacity(encoded_pixels * 3);
    let mut jpeg_buf = Vec::with_capacity(encoded_pixels / 2);

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
    let (source_width, source_height, stride) = frame_geometry(frame.len(), width, height)?;
    if stride < source_width * 4 {
        bail!("unexpected frame stride");
    }

    let encoded_width = source_width.div_ceil(SCALE_DIVISOR);
    let encoded_height = source_height.div_ceil(SCALE_DIVISOR);
    rgb_buf.clear();
    rgb_buf.reserve(encoded_width * encoded_height * 3);
    for y in (0..source_height).step_by(SCALE_DIVISOR) {
        let row = &frame[y * stride..(y * stride + source_width * 4)];
        for x in (0..source_width).step_by(SCALE_DIVISOR) {
            let offset = x * 4;
            rgb_buf.extend_from_slice(&[row[offset + 2], row[offset + 1], row[offset]]);
        }
    }
    overlay_cursor(rgb_buf, encoded_width, encoded_height);

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

fn overlay_cursor(rgb: &mut [u8], width: usize, height: usize) {
    let Some(cursor) = cursor_position() else {
        return;
    };
    let x = cursor.x.div_euclid(SCALE_DIVISOR as i32);
    let y = cursor.y.div_euclid(SCALE_DIVISOR as i32);
    const CURSOR_DOT_RADIUS: i32 = 9;
    const CURSOR_DOT_COLOR: [u8; 3] = [255, 0, 0];

    for dy in -CURSOR_DOT_RADIUS..=CURSOR_DOT_RADIUS {
        for dx in -CURSOR_DOT_RADIUS..=CURSOR_DOT_RADIUS {
            if dx * dx + dy * dy <= CURSOR_DOT_RADIUS * CURSOR_DOT_RADIUS {
                draw_cursor_pixel(rgb, width, height, x + dx, y + dy, CURSOR_DOT_COLOR);
            }
        }
    }
}

fn draw_cursor_pixel(rgb: &mut [u8], width: usize, height: usize, x: i32, y: i32, color: [u8; 3]) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as usize;
    let y = y as usize;
    if x >= width || y >= height {
        return;
    }
    let offset = (y * width + x) * 3;
    rgb[offset..offset + 3].copy_from_slice(&color);
}

fn frame_geometry(
    frame_len: usize,
    display_width: usize,
    display_height: usize,
) -> Result<(usize, usize, usize)> {
    if display_width == 0 || display_height == 0 {
        bail!("invalid screen size");
    }

    let nominal_bytes = display_width * display_height * 4;
    if frame_len >= nominal_bytes {
        let scale = (frame_len as f64 / nominal_bytes as f64).sqrt();
        if scale > 1.05 {
            let scaled_width = (display_width as f64 * scale).round() as usize;
            let scaled_height = (display_height as f64 * scale).round() as usize;
            if scaled_width > display_width
                && scaled_height > display_height
                && scaled_height > 0
                && frame_len / scaled_height >= scaled_width * 4
            {
                return Ok((scaled_width, scaled_height, frame_len / scaled_height));
            }
        }
    }

    Ok((display_width, display_height, frame_len / display_height))
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

fn log_headless_event(message: &str) {
    let log_dir = std::env::var_os("PROGRAMDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"))
        .join("RemoteControl");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("client_headless.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|_| "unknown-time".to_string());
        let _ = writeln!(file, "[{timestamp}] {message}");
    }
}

#[cfg(target_os = "windows")]
fn set_autostart_enabled() -> Result<()> {
    if startup_task_exists(STARTUP_TASK_NAME).unwrap_or(false) {
        return Ok(());
    }
    let exe = std::env::current_exe().context("current_exe failed")?;
    set_interactive_user_startup_task(STARTUP_TASK_NAME, &exe)
        .context("create current-user scheduled task failed")?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_autostart_enabled() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_registry_autostart_enabled() -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    let exe = std::env::current_exe().context("current_exe failed")?;
    run_key.set_value(
        REGISTRY_VALUE_NAME,
        &format!("\"{}\"", exe.to_string_lossy()),
    )?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_registry_autostart_enabled() -> Result<()> {
    Ok(())
}
