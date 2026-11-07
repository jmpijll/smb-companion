//! Windows integration that keeps the companion out of the game's way:
//! one instance, low priority, tray-first startup and window control that
//! never activates the window unless the player asked for it.
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::atomic::{AtomicIsize, Ordering},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND, INVALID_HANDLE_VALUE, RECT,
        SYSTEMTIME,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Registry::{
            RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
            RRF_RT_REG_SZ,
        },
        SystemInformation::GetLocalTime,
        Threading::{
            CreateEventW, CreateMutexW, GetCurrentProcess, OpenEventW, SetEvent, SetPriorityClass,
            WaitForSingleObject, BELOW_NORMAL_PRIORITY_CLASS, EVENT_MODIFY_STATE, INFINITE,
        },
    },
    UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, GetWindowRect, IsIconic, SetForegroundWindow, SetWindowPos,
        ShowWindow, SystemParametersInfoW, ASFW_ANY, SPI_GETWORKAREA, SWP_HIDEWINDOW,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWMINNOACTIVE,
    },
};

pub const APP_NAME: &str = "SMB LoL Companion";
const MUTEX: &str = "Local\\smb-lol-companion.instance";
const SHOW_EVENT: &str = "Local\\smb-lol-companion.show";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Off-screen origin used when starting in the tray, so the first frame that
/// eframe insists on showing is never visible on any monitor.
pub const OFFSCREEN: f32 = -32000.;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

static WINDOW: AtomicIsize = AtomicIsize::new(0);

pub fn set_window(hwnd: isize) {
    WINDOW.store(hwnd, Ordering::SeqCst);
}
fn hwnd() -> Option<HWND> {
    match WINDOW.load(Ordering::SeqCst) {
        0 => None,
        h => Some(h as HWND),
    }
}

/// Hides the window without touching focus. Works while egui is not painting.
pub fn hide_window() {
    if let Some(h) = hwnd() {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
}

/// Hides the window and moves it from the off-screen start position to the
/// centre of the primary work area, still without activating anything.
pub fn park_window() {
    let Some(h) = hwnd() else { return };
    unsafe {
        let mut win: RECT = std::mem::zeroed();
        let mut work: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut win);
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
        let (w, ht) = (win.right - win.left, win.bottom - win.top);
        let x = work.left + ((work.right - work.left) - w).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - ht).max(0) / 2;
        SetWindowPos(
            h,
            null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
        );
    }
}

/// Shows the window because the player explicitly asked for it (tray click,
/// tray menu, or launching the app again).
pub fn show_window() {
    let Some(h) = hwnd() else { return };
    unsafe {
        let mut win: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut win);
        if (win.left as f32) < OFFSCREEN / 2. {
            park_window();
        }
        ShowWindow(
            h,
            if IsIconic(h) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        SetForegroundWindow(h);
    }
}

/// Fallback when no tray icon exists: minimized, never activated.
pub fn minimize_quietly() {
    if let Some(h) = hwnd() {
        unsafe { ShowWindow(h, SW_SHOWMINNOACTIVE) };
    }
}

/// Returns false when another copy is already running. Unless `quiet`, the
/// running copy is asked to show its window (the player launched us again).
pub fn claim_single_instance(quiet: bool) -> bool {
    unsafe {
        let name = wide(MUTEX);
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            if !quiet {
                let ev = OpenEventW(EVENT_MODIFY_STATE, 0, wide(SHOW_EVENT).as_ptr());
                if !ev.is_null() {
                    AllowSetForegroundWindow(ASFW_ANY);
                    SetEvent(ev);
                    CloseHandle(ev);
                }
            }
            return false;
        }
        // The mutex handle is intentionally kept for the life of the process.
        let ev = CreateEventW(null(), 0, 0, wide(SHOW_EVENT).as_ptr());
        if !ev.is_null() {
            let ev = ev as isize;
            std::thread::spawn(move || loop {
                if WaitForSingleObject(ev as _, INFINITE) != 0 {
                    break;
                }
                show_window();
            });
        }
        true
    }
}

/// The companion never competes with the game for CPU time.
pub fn lower_priority() {
    unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
}

/// True when the League game process (not just the client) is running.
pub fn game_running() -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len])
                .eq_ignore_ascii_case("League of Legends.exe")
            {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

fn autostart_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

fn read_run_value() -> Option<String> {
    unsafe {
        let mut buf = vec![0u16; 2048];
        let mut size = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(APP_NAME).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        );
        if rc != 0 {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Whether Windows starts this exact executable (in the tray) at sign-in.
pub fn autostart_enabled() -> bool {
    read_run_value().is_some_and(|v| Some(v) == autostart_command())
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let rc = unsafe {
        if on {
            let cmd = autostart_command().ok_or("Could not locate the app's own executable.")?;
            let data = wide(&cmd);
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(APP_NAME).as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(APP_NAME).as_ptr(),
            ) {
                2 => 0, // ERROR_FILE_NOT_FOUND: already off
                rc => rc,
            }
        }
    };
    if rc == 0 {
        Ok(())
    } else {
        Err("Windows did not accept the start-up setting.".into())
    }
}

/// Local wall-clock time as HH:MM.
pub fn local_time() -> String {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

/// %APPDATA%\SMB\LoL Companion — survives moving or updating the exe.
pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Into::into))
        })
        .unwrap_or_default()
        .join("SMB")
        .join("LoL Companion")
}
