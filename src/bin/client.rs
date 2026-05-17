#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use eframe::egui;
use jpeg_encoder::{ColorType, Encoder};
use remote_control::config::TARGET_FPS;
use remote_control::protocol::{ClientHello, write_frame, write_hello};
use remote_control::ui_fonts::install_cjk_font;
#[cfg(not(target_os = "windows"))]
use scrap::{Capturer, Display};
#[cfg(not(target_os = "windows"))]
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::time::Instant;

#[cfg(target_os = "windows")]
use windows_capture::capture::{Context as WgcContext, GraphicsCaptureApiHandler};
#[cfg(target_os = "windows")]
use windows_capture::graphics_capture_api::InternalCaptureControl;
#[cfg(target_os = "windows")]
use windows_capture::monitor::Monitor;
#[cfg(target_os = "windows")]
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::HKEY_CURRENT_USER;

const SERVER_ADDR: &str = "172.20.20.8:5000";
const JPEG_QUALITY: u8 = 45;
const SCALE_DIVISOR: usize = 2;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Remote Monitor Client",
        options,
        Box::new(|cc| Ok(Box::new(ClientApp::new(cc)))),
    )
}

struct ClientApp {
    consent_checked: bool,
    autostart_checked: bool,
    sharing: bool,
    status: String,
    stop_signal: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
    rx: Option<Receiver<ClientEvent>>,
    font_status: String,
}

impl ClientApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let font_status = match install_cjk_font(&cc.egui_ctx) {
            Ok(path) => format!("中文字体: {path}"),
            Err(e) => format!("中文字体加载失败: {e}"),
        };
        Self {
            consent_checked: false,
            autostart_checked: get_autostart_enabled().unwrap_or(false),
            sharing: false,
            status: "未开始共享".to_string(),
            stop_signal: None,
            worker: None,
            rx: None,
            font_status,
        }
    }

    fn start_sharing(&mut self) {
        if self.sharing {
            return;
        }
        if !self.consent_checked {
            self.status = "请先勾选用户授权".to_string();
            return;
        }

        let stop_signal = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop_signal);
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            if let Err(e) = run_stream_loop(worker_stop, tx.clone()) {
                let _ = tx.send(ClientEvent::Status(format!("推流线程退出: {e:#}")));
            }
            let _ = tx.send(ClientEvent::Stopped);
        });

        self.stop_signal = Some(stop_signal);
        self.worker = Some(worker);
        self.rx = Some(rx);
        self.sharing = true;
        self.status = "正在启动推流...".to_string();
    }

    fn stop_sharing(&mut self) {
        if let Some(signal) = &self.stop_signal {
            signal.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.stop_signal = None;
        self.rx = None;
        self.sharing = false;
        self.status = "已停止共享".to_string();
    }

    fn drain_events(&mut self) {
        let mut should_mark_stopped = false;
        if let Some(rx) = &self.rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ClientEvent::Status(msg) => self.status = msg,
                    ClientEvent::Stopped => should_mark_stopped = true,
                }
            }
        }
        if should_mark_stopped {
            self.sharing = false;
            self.worker = None;
            self.stop_signal = None;
        }
    }
}

impl Drop for ClientApp {
    fn drop(&mut self) {
        self.stop_sharing();
    }
}

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        ctx.request_repaint_after(Duration::from_millis(50));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("远程桌面共享客户端（Windows 优化）");
            ui.separator();
            ui.label("截图引擎: Windows Graphics Capture (Windows 10/11)");
            ui.label(format!("服务器地址（硬编码）: {SERVER_ADDR}"));
            ui.separator();

            ui.checkbox(
                &mut self.consent_checked,
                "我已获得当前设备使用者授权，同意开始桌面共享",
            );

            let autostart_changed = ui
                .checkbox(
                    &mut self.autostart_checked,
                    "开机自动启动客户端（当前用户，注册表 Run）",
                )
                .changed();
            if autostart_changed {
                match set_autostart_enabled(self.autostart_checked) {
                    Ok(()) => {}
                    Err(e) => {
                        self.status = format!("设置开机启动失败: {e:#}");
                        self.autostart_checked = get_autostart_enabled().unwrap_or(false);
                    }
                }
            }

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.sharing, egui::Button::new("开始共享"))
                    .clicked()
                {
                    self.start_sharing();
                }
                if ui
                    .add_enabled(self.sharing, egui::Button::new("停止共享"))
                    .clicked()
                {
                    self.stop_sharing();
                }
            });

            ui.separator();
            ui.label(format!("状态: {}", self.status));
            ui.label(&self.font_status);
        });
    }
}

enum ClientEvent {
    Status(String),
    Stopped,
}

fn run_stream_loop(stop: Arc<AtomicBool>, tx: Sender<ClientEvent>) -> Result<()> {
    while !stop.load(Ordering::Relaxed) {
        let _ = tx.send(ClientEvent::Status(format!("连接服务端 {SERVER_ADDR} ...")));
        match run_stream_session(&stop) {
            Ok(()) => {
                if !stop.load(Ordering::Relaxed) {
                    let _ = tx.send(ClientEvent::Status("连接已关闭，准备重连...".to_string()));
                }
            }
            Err(e) => {
                if !stop.load(Ordering::Relaxed) {
                    let _ = tx.send(ClientEvent::Status(format!("连接异常: {e:#}，2秒后重连")));
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_stream_session(stop: &Arc<AtomicBool>) -> Result<()> {
    #[derive(Clone)]
    struct WgcFlags {
        stop: Arc<AtomicBool>,
    }

    struct WgcHandler {
        stop: Arc<AtomicBool>,
        stream: TcpStream,
        frame_interval: Duration,
        last_frame_sent: Instant,
        rgb_buf: Vec<u8>,
        jpeg_buf: Vec<u8>,
        rgba_nopad: Vec<u8>,
    }

    impl GraphicsCaptureApiHandler for WgcHandler {
        type Flags = WgcFlags;
        type Error = anyhow::Error;

        fn new(ctx: WgcContext<Self::Flags>) -> std::result::Result<Self, Self::Error> {
            let mut stream = TcpStream::connect(SERVER_ADDR)
                .with_context(|| format!("connect failed: {SERVER_ADDR}"))?;
            stream.set_nodelay(true)?;
            write_hello(
                &mut stream,
                &ClientHello {
                    client_name: client_name(),
                },
            )?;

            Ok(Self {
                stop: ctx.flags.stop,
                stream,
                frame_interval: Duration::from_millis((1000 / TARGET_FPS).max(1)),
                last_frame_sent: Instant::now() - Duration::from_secs(1),
                rgb_buf: Vec::new(),
                jpeg_buf: Vec::new(),
                rgba_nopad: Vec::new(),
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut windows_capture::frame::Frame,
            capture_control: InternalCaptureControl,
        ) -> std::result::Result<(), Self::Error> {
            if self.stop.load(Ordering::Relaxed) {
                let _ = capture_control.stop();
                return Ok(());
            }

            let now = Instant::now();
            if now.duration_since(self.last_frame_sent) < self.frame_interval {
                return Ok(());
            }
            self.last_frame_sent = now;

            let frame_buffer = frame.buffer()?;
            let width = frame_buffer.width() as usize;
            let height = frame_buffer.height() as usize;

            let rgba = frame_buffer.as_nopadding_buffer(&mut self.rgba_nopad);
            encode_jpeg_rgba_reuse(rgba, width, height, &mut self.rgb_buf, &mut self.jpeg_buf)?;
            write_frame(&mut self.stream, &self.jpeg_buf)?;
            Ok(())
        }
    }

    let monitor = Monitor::primary()?;
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        WgcFlags {
            stop: Arc::clone(stop),
        },
    );

    WgcHandler::start(settings)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn run_stream_session(stop: &Arc<AtomicBool>) -> Result<()> {
    let mut stream = TcpStream::connect(SERVER_ADDR)
        .with_context(|| format!("connect failed: {SERVER_ADDR}"))?;
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

    let mut rgb_buf = vec![0_u8; width * height * 3];
    let mut jpeg_buf = Vec::with_capacity(width * height / 3);

    while !stop.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let frame = loop {
            match capturer.frame() {
                Ok(frame) => break frame,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
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

#[cfg(target_os = "windows")]
fn encode_jpeg_rgba_reuse(
    rgba: &[u8],
    width: usize,
    height: usize,
    rgb_buf: &mut Vec<u8>,
    jpeg_buf: &mut Vec<u8>,
) -> Result<()> {
    if rgba.len() < width * height * 4 {
        bail!("invalid rgba frame size");
    }

    let encoded_width = width.div_ceil(SCALE_DIVISOR);
    let encoded_height = height.div_ceil(SCALE_DIVISOR);
    rgb_buf.clear();
    rgb_buf.reserve(encoded_width * encoded_height * 3);
    for y in (0..height).step_by(SCALE_DIVISOR) {
        let row_start = y * width * 4;
        for x in (0..width).step_by(SCALE_DIVISOR) {
            let offset = row_start + x * 4;
            rgb_buf.extend_from_slice(&[rgba[offset], rgba[offset + 1], rgba[offset + 2]]);
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

#[cfg(not(target_os = "windows"))]
fn encode_jpeg_bgra_reuse(
    frame: &[u8],
    width: usize,
    height: usize,
    rgb_buf: &mut Vec<u8>,
    jpeg_buf: &mut Vec<u8>,
) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("invalid screen size");
    }
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
fn set_autostart_enabled(enabled: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    const VALUE_NAME: &str = "RemoteMonitorClientCompliant";
    if enabled {
        let exe = std::env::current_exe().context("current_exe failed")?;
        run_key.set_value(VALUE_NAME, &exe.to_string_lossy().to_string())?;
    } else {
        let _ = run_key.delete_value(VALUE_NAME);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_autostart_enabled(_enabled: bool) -> Result<()> {
    bail!("autostart is only supported on Windows");
}

#[cfg(target_os = "windows")]
fn get_autostart_enabled() -> Result<bool> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    const VALUE_NAME: &str = "RemoteMonitorClientCompliant";
    let value: String = run_key.get_value(VALUE_NAME)?;
    let exe = std::env::current_exe().context("current_exe failed")?;
    Ok(value.eq_ignore_ascii_case(&exe.to_string_lossy()))
}

#[cfg(not(target_os = "windows"))]
fn get_autostart_enabled() -> Result<bool> {
    Ok(false)
}
