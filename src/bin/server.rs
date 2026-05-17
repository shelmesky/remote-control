use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use eframe::egui;
use remote_control::config::{SERVER_BIND_ADDR, SERVER_TARGET_ADDR};
use remote_control::protocol::{read_and_validate_hello, read_frame};
use remote_control::ui_fonts::install_cjk_font;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Remote Monitor Server",
        options,
        Box::new(|cc| Ok(Box::new(ServerApp::new(cc)))),
    )
}

struct ServerApp {
    rx: Receiver<ServerEvent>,
    status: String,
    client_name: Option<String>,
    frame_count: u64,
    texture: Option<egui::TextureHandle>,
    font_status: String,
}

impl ServerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let font_status = match install_cjk_font(&cc.egui_ctx) {
            Ok(path) => format!("中文字体: {path}"),
            Err(e) => format!("中文字体加载失败: {e}"),
        };
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            if let Err(e) = listener_loop(tx.clone()) {
                let _ = tx.send(ServerEvent::Status(format!("监听线程退出: {e:#}")));
            }
        });

        Self {
            rx,
            status: format!("正在监听 {SERVER_BIND_ADDR}"),
            client_name: None,
            frame_count: 0,
            texture: None,
            font_status,
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ServerEvent::Status(msg) => self.status = msg,
                ServerEvent::Connected(name) => {
                    self.client_name = Some(name.clone());
                    self.status = format!("客户端已连接: {name}");
                    self.frame_count = 0;
                }
                ServerEvent::Disconnected => {
                    self.status = "客户端已断开，等待新连接".to_string();
                    self.client_name = None;
                    self.texture = None;
                }
                ServerEvent::Frame(bytes) => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let pixels = rgba.as_raw();
                        let color = egui::ColorImage::from_rgba_unmultiplied(size, pixels);
                        if let Some(texture) = &mut self.texture {
                            texture.set(color, egui::TextureOptions::LINEAR);
                        } else {
                            self.texture = Some(ctx.load_texture(
                                "remote-frame",
                                color,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.frame_count += 1;
                    }
                }
            }
        }
    }
}

impl eframe::App for ServerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("远程桌面监视服务端");
            ui.separator();
            ui.label(format!("监听地址: {SERVER_BIND_ADDR}"));
            ui.label(format!(
                "客户端应连接地址: {SERVER_TARGET_ADDR} (或服务器实际 IP)"
            ));
            ui.label(format!("状态: {}", self.status));
            ui.label(&self.font_status);
            if let Some(name) = &self.client_name {
                ui.label(format!("客户端: {name}"));
            }
            ui.label(format!("已接收帧数: {}", self.frame_count));
            ui.separator();

            let available = ui.available_size();
            if let Some(texture) = &self.texture {
                let tex_size = texture.size_vec2();
                let scale = (available.x / tex_size.x)
                    .min(available.y / tex_size.y)
                    .max(0.1);
                let draw_size = tex_size * scale;
                ui.image((texture.id(), draw_size));
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("等待客户端推流...");
                });
            }
        });
    }
}

enum ServerEvent {
    Status(String),
    Connected(String),
    Disconnected,
    Frame(Vec<u8>),
}

fn listener_loop(tx: Sender<ServerEvent>) -> Result<()> {
    let listener = TcpListener::bind(SERVER_BIND_ADDR)?;
    let _ = tx.send(ServerEvent::Status(format!("正在监听 {SERVER_BIND_ADDR}")));

    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(stream) => stream,
            Err(e) => {
                let _ = tx.send(ServerEvent::Status(format!("accept failed: {e}")));
                continue;
            }
        };

        match read_and_validate_hello(&mut stream) {
            Ok(hello) => {
                let _ = tx.send(ServerEvent::Connected(hello.client_name));
            }
            Err(e) => {
                let _ = tx.send(ServerEvent::Status(format!("握手失败: {e:#}")));
                continue;
            }
        }

        loop {
            match read_frame(&mut stream) {
                Ok(frame) => {
                    if tx.send(ServerEvent::Frame(frame)).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => {
                    let _ = tx.send(ServerEvent::Status(format!("接收中断: {e:#}")));
                    let _ = tx.send(ServerEvent::Disconnected);
                    break;
                }
            }
        }
    }
    Ok(())
}
