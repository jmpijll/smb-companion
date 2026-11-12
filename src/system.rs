//! Windows integration that keeps the companion out of the game's way:
//! one instance, low priority, tray-first startup and window control that
//! never activates the window unless the player asked for it.
use std::{
    ffi::OsStr,
    io::Write,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
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
            CreateEventW, CreateMutexW, GetCurrentProcess, OpenEventW, OpenMutexW, OpenProcess,
            QueryFullProcessImageNameW, SetEvent, SetPriorityClass, WaitForMultipleObjects,
            BELOW_NORMAL_PRIORITY_CLASS, EVENT_MODIFY_STATE, INFINITE,
            PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZATION_SYNCHRONIZE,
        },
    },
    UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible, SetForegroundWindow,
        SetWindowPos, ShowWindow, SystemParametersInfoW, ASFW_ANY, SPI_GETWORKAREA, SWP_HIDEWINDOW,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, SW_SHOW,
        SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
    },
};

pub const APP_NAME: &str = "SMB Companion";
/// Name of the "Start with Windows" value. Never change it: the installer
/// writes the same name, and older versions left it under this one.
const RUN_VALUE: &str = "SMB LoL Companion";
const MUTEX: &str = "Local\\smb-lol-companion.instance";
const SHOW_EVENT: &str = "Local\\smb-lol-companion.show";
const QUIT_EVENT: &str = "Local\\smb-lol-companion.quit";
/// Next to the exe, this file switches the app to portable mode.
pub const PORTABLE_MARKER: &str = "smb-lol.portable";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Off-screen origin used when starting in the tray, so the first frame that
/// eframe insists on showing is never visible on any monitor.
pub const OFFSCREEN: f32 = -32000.;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static QUIT: AtomicBool = AtomicBool::new(false);

/// Set when the installer/uninstaller (via `--quit`) asked this copy to exit.
pub fn quit_requested() -> bool {
    QUIT.load(Ordering::SeqCst)
}

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
        let show = CreateEventW(null(), 0, 0, wide(SHOW_EVENT).as_ptr());
        let quit = CreateEventW(null(), 0, 0, wide(QUIT_EVENT).as_ptr());
        if !show.is_null() && !quit.is_null() {
            let events = [show as isize, quit as isize];
            std::thread::spawn(move || loop {
                let handles = events.map(|h| h as _);
                match WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) {
                    0 => show_window(),
                    1 => {
                        QUIT.store(true, Ordering::SeqCst);
                        wake_for_quit();
                    }
                    _ => break,
                }
            });
        }
        true
    }
}

/// egui only runs while the window is shown, so a hidden window is shown
/// off-screen (never activated) for the few frames it takes to shut down and
/// remove the tray icon cleanly.
fn wake_for_quit() {
    let Some(h) = hwnd() else {
        std::process::exit(0)
    };
    unsafe {
        SetWindowPos(
            h,
            null_mut(),
            OFFSCREEN as i32,
            OFFSCREEN as i32,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_SHOWNOACTIVATE);
        }
    }
    // Last resort if the UI does not get to it.
    std::thread::sleep(Duration::from_secs(5));
    std::process::exit(0);
}

/// `--quit`: asks a running copy to exit and waits up to `timeout` for it.
/// Returns true when no copy is running any more.
pub fn quit_running_instance(timeout: Duration) -> bool {
    unsafe {
        let running = || {
            let m = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(MUTEX).as_ptr());
            if m.is_null() {
                false
            } else {
                CloseHandle(m);
                true
            }
        };
        if !running() {
            return true;
        }
        let ev = OpenEventW(EVENT_MODIFY_STATE, 0, wide(QUIT_EVENT).as_ptr());
        if ev.is_null() {
            return false; // an older build without quit support
        }
        SetEvent(ev);
        CloseHandle(ev);
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !running()
    }
}

/// The companion never competes with the game for CPU time.
pub fn lower_priority() {
    unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
}

/// Process ID of the first running process with this executable name.
fn find_process(exe: &str) -> Option<u32> {
    find_processes(exe).into_iter().next()
}

/// Every running process with one of these executable names.
fn find_processes(exe: &str) -> Vec<u32> {
    let mut found = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                found.push(entry.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    found
}

/// Full executable path of a running process, if Windows lets us read it.
pub fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Running Java processes with their executable paths (None when unreadable).
pub fn java_processes() -> Vec<(u32, Option<PathBuf>)> {
    ["java.exe", "javaw.exe"]
        .iter()
        .flat_map(|exe| find_processes(exe))
        .map(|pid| (pid, process_path(pid)))
        .collect()
}

/// True when the League game process (not just the client) is running.
pub fn game_running() -> bool {
    find_process("League of Legends.exe").is_some()
}

/// Whether a process with this executable name is running.
pub fn process_running(exe: &str) -> bool {
    find_process(exe).is_some()
}

/// Minecraft from Modrinth is running: pack files must not change now.
/// Other Java apps (IDEs, build tools) no longer hold updates back.
pub fn minecraft_running() -> bool {
    crate::minecraft::modrinth::game_running()
}

/// Folder of the running League client, wherever it is installed. Its
/// lockfile lives next to it.
pub fn league_client_dir() -> Option<PathBuf> {
    ["LeagueClientUx.exe", "LeagueClient.exe"]
        .iter()
        .find_map(|exe| find_process(exe))
        .and_then(process_path)
        .and_then(|exe| exe.parent().map(PathBuf::from))
}

/// Whether the player currently has the window on screen.
pub fn window_visible() -> bool {
    let Some(h) = hwnd() else { return false };
    unsafe {
        let mut win: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut win);
        IsWindowVisible(h) != 0 && IsIconic(h) == 0 && (win.left as f32) > OFFSCREEN / 2.
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
            wide(RUN_VALUE).as_ptr(),
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
                wide(RUN_VALUE).as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(RUN_VALUE).as_ptr(),
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

fn now() -> SYSTEMTIME {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t
}

/// Local wall-clock time as HH:MM.
pub fn local_time() -> String {
    let t = now();
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

/// One line in `activity.log` next to the settings, so "why didn't it apply?"
/// has an answer without asking the player anything. Bounded: past 256 KB the
/// file rolls over to `activity.old.log`.
pub fn log(message: &str) {
    if cfg!(test) {
        return;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = data_dir();
    let path = dir.join("activity.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, dir.join("activity.old.log"));
    }
    let _ = std::fs::create_dir_all(&dir);
    let t = now();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(
            file,
            "{}-{:02}-{:02} {:02}:{:02}:{:02}  {}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, message
        );
    }
}

/// The exe's folder when the portable marker sits next to it.
pub fn portable_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        dir.join(PORTABLE_MARKER).is_file().then_some(dir)
    })
    .clone()
}

/// Installed by the setup (its uninstaller sits next to the exe), so the
/// app may update itself with a newer setup.
pub fn installed() -> bool {
    portable_dir().is_none()
        && std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").is_file()))
            .unwrap_or(false)
}

/// Portable: the exe's own folder. Installed: %APPDATA%\SMB\LoL Companion,
/// which survives updates and moving the exe.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = portable_dir() {
        return dir;
    }
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
