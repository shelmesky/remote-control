#[cfg(target_os = "windows")]
pub fn enable_dpi_awareness() {
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;

    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            let _ = SetProcessDPIAware();
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn enable_dpi_awareness() {}

#[derive(Clone, Copy)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
}

#[cfg(target_os = "windows")]
pub fn cursor_position() -> Option<CursorPosition> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CURSOR_SHOWING, CURSORINFO, GetCursorInfo};

    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };

    unsafe {
        if GetCursorInfo(&mut info) == 0 || info.flags & CURSOR_SHOWING == 0 {
            return None;
        }
    }

    Some(CursorPosition {
        x: info.ptScreenPos.x,
        y: info.ptScreenPos.y,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn cursor_position() -> Option<CursorPosition> {
    None
}
