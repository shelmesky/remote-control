use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub fn install_cjk_font(ctx: &egui::Context) -> Result<String> {
    let (path, bytes) = load_font_bytes()?;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "cjk_fallback".to_owned(),
        FontData::from_owned(bytes).into(),
    );

    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "cjk_fallback".to_owned());
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
        family.insert(0, "cjk_fallback".to_owned());
    }
    ctx.set_fonts(fonts);
    Ok(path.display().to_string())
}

fn load_font_bytes() -> Result<(PathBuf, Vec<u8>)> {
    for path in candidate_font_paths() {
        if let Ok(bytes) = fs::read(&path) {
            return Ok((path, bytes));
        }
    }
    bail!("未找到可用中文字体，请安装 Noto Sans CJK 或 WenQuanYi")
}

#[cfg(target_os = "linux")]
fn candidate_font_paths() -> Vec<PathBuf> {
    vec![
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf".into(),
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc".into(),
        "/usr/share/fonts/truetype/noto/NotoSansCJKsc-Regular.otf".into(),
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".into(),
        "/usr/share/fonts/wenquanyi/wqy-zenhei/wqy-zenhei.ttc".into(),
    ]
}

#[cfg(target_os = "windows")]
fn candidate_font_paths() -> Vec<PathBuf> {
    vec![
        r"C:\Windows\Fonts\msyh.ttc".into(),
        r"C:\Windows\Fonts\msyhbd.ttc".into(),
        r"C:\Windows\Fonts\simhei.ttf".into(),
        r"C:\Windows\Fonts\simsun.ttc".into(),
    ]
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn candidate_font_paths() -> Vec<PathBuf> {
    Vec::new()
}
