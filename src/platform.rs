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

#[cfg(target_os = "windows")]
pub struct SingleInstanceGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn acquire_single_instance(name: &str) -> Option<SingleInstanceGuard> {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, SetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name = format!("Global\\{name}");
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SetLastError(0);
        let handle = CreateMutexW(std::ptr::null_mut(), 1, wide_name.as_ptr());
        if handle.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
            return None;
        }
        Some(SingleInstanceGuard { handle })
    }
}

#[cfg(not(target_os = "windows"))]
pub struct SingleInstanceGuard;

#[cfg(not(target_os = "windows"))]
pub fn acquire_single_instance(_name: &str) -> Option<SingleInstanceGuard> {
    Some(SingleInstanceGuard)
}

#[cfg(target_os = "windows")]
pub fn set_interactive_user_startup_task(
    task_name: &str,
    exe_path: &std::path::Path,
) -> std::io::Result<()> {
    let task_command = format!("\"{}\"", exe_path.display());
    let status = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            task_name,
            "/SC",
            "MINUTE",
            "/MO",
            "1",
            "/TR",
            &task_command,
            "/F",
        ])
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "schtasks /Create failed with exit code {:?}",
            status.code()
        )));
    }

    set_interactive_user_task_settings(task_name, exe_path)
}

#[cfg(target_os = "windows")]
fn set_interactive_user_task_settings(
    task_name: &str,
    exe_path: &std::path::Path,
) -> std::io::Result<()> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$execute = $args[1]
$workingDirectory = $args[2]
$action = New-ScheduledTaskAction -Execute $execute -WorkingDirectory $workingDirectory
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Seconds 0)
Set-ScheduledTask -TaskName $args[0] -Action $action -Settings $settings | Out-Null
"#;
    let working_directory = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
            task_name,
            &exe_path.to_string_lossy(),
            &working_directory.to_string_lossy(),
        ])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::other(format!(
            "Set-ScheduledTask failed with exit code {:?}: {}",
            output.status.code(),
            stderr.trim()
        )))
    }
}

#[cfg(target_os = "windows")]
pub fn delete_startup_task(task_name: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", task_name, "/F"])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "schtasks /Delete failed with exit code {:?}",
            status.code()
        )))
    }
}

#[cfg(target_os = "windows")]
pub fn startup_task_exists(task_name: &str) -> std::io::Result<bool> {
    let status = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", task_name])
        .status()?;
    Ok(status.success())
}

#[cfg(not(target_os = "windows"))]
pub fn set_interactive_user_startup_task(
    _task_name: &str,
    _exe_path: &std::path::Path,
) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "scheduled startup tasks are only supported on Windows",
    ))
}

#[cfg(not(target_os = "windows"))]
pub fn delete_startup_task(_task_name: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn startup_task_exists(_task_name: &str) -> std::io::Result<bool> {
    Ok(false)
}
