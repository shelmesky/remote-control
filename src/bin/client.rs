use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use eframe::egui;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use remote_control::config::{SERVER_TARGET_ADDR, TARGET_FPS};
use remote_control::protocol::{ClientHello, write_frame, write_hello};
use remote_control::ui_fonts::install_cjk_font;
use scrap::{Capturer, Display};

#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::HKEY_CURRENT_USER;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Remote Monitor Client (Compliant)",
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
                    ClientEvent::Stopped => {
                        should_mark_stopped = true;
                    }
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
            ui.heading("远程桌面共享客户端（合规模式）");
            ui.separator();
            ui.label("客户端仅在用户明确授权后推流，且界面持续可见。");
            ui.label(format!("服务器地址（固定）: {SERVER_TARGET_ADDR}"));
            ui.separator();

            ui.checkbox(
                &mut self.consent_checked,
                "我已获得当前设备使用者授权，同意开始桌面共享",
            );

            let autostart_changed = ui
                .checkbox(
                    &mut self.autostart_checked,
                    "开机自动启动客户端（当前用户）",
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
            ui.colored_label(
                if self.sharing {
                    egui::Color32::LIGHT_GREEN
                } else {
                    egui::Color32::LIGHT_RED
                },
                format!("状态: {}", self.status),
            );
            ui.label(&self.font_status);
            if self.sharing {
                ui.label("提示：当前正在持续采集桌面并发送到服务端。");
            }
        });
    }
}

enum ClientEvent {
    Status(String),
    Stopped,
}

fn run_stream_loop(stop: Arc<AtomicBool>, tx: Sender<ClientEvent>) -> Result<()> {
    while !stop.load(Ordering::Relaxed) {
        let _ = tx.send(ClientEvent::Status(format!(
            "连接服务端 {SERVER_TARGET_ADDR} ..."
        )));
        match run_stream_session(&stop, &tx) {
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
    let _ = tx.send(ClientEvent::Stopped);
    Ok(())
}

fn run_stream_session(stop: &Arc<AtomicBool>, tx: &Sender<ClientEvent>) -> Result<()> {
    let mut stream = TcpStream::connect(SERVER_TARGET_ADDR)
        .with_context(|| format!("connect failed: {SERVER_TARGET_ADDR}"))?;
    stream.set_nodelay(true)?;

    let client_name = format!(
        "{}@{}",
        std::env::var("USERNAME").unwrap_or_else(|_| "unknown-user".to_string()),
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".to_string())
    );
    write_hello(&mut stream, &ClientHello { client_name })?;
    let _ = tx.send(ClientEvent::Status("已连接，开始推流".to_string()));

    let display = Display::primary().context("unable to get primary display")?;
    let mut capturer = Capturer::new(display).context("unable to create screen capturer")?;
    let width = capturer.width();
    let height = capturer.height();

    let frame_interval = Duration::from_millis((1000 / TARGET_FPS).max(1));

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

        let jpeg = encode_jpeg_bgra(&frame, width, height).context("jpeg encode failed")?;
        write_frame(&mut stream, &jpeg).context("send frame failed")?;

        let elapsed = tick.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }
    Ok(())
}

fn encode_jpeg_bgra(frame: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    if height == 0 || width == 0 {
        bail!("invalid screen size");
    }
    let stride = frame.len() / height;
    if stride < width * 4 {
        bail!("unexpected frame stride");
    }

    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row = &frame[y * stride..(y * stride + width * 4)];
        for px in row.chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }

    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 70);
    encoder.encode(&rgb, width as u32, height as u32, ExtendedColorType::Rgb8)?;
    Ok(out)
}

#[cfg(target_os = "windows")]
fn set_autostart_enabled(enabled: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    const VALUE_NAME: &str = "RemoteMonitorClientCompliant";

    if enabled {
        let exe = std::env::current_exe().context("current_exe failed")?;
        let value = exe.to_string_lossy().to_string();
        run_key.set_value(VALUE_NAME, &value)?;
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
