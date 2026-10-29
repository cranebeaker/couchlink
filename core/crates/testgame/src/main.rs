//! testgame: a tiny window app that stands in for a real game in automated tests.
//!
//! It opens one ordinary window (with a title bar, so a launcher has something to strip) and writes
//! a JSON status file every 200 ms: pid, seat, window rect, style, whether it is the foreground
//! window, the arguments it got and the environment variables it was given.
//!
//! usage: testgame [--seat N] [--player NAME] [--out FILE] [--exit-after-ms N] [--hang-after-ms N]
//!        testgame --listen PORT [--out FILE]      (no window; listens on 127.0.0.1:PORT, a stand-in server)
//!
//! Also read once at start and reported as `"files"` and `"registry"` in the status file, so a test can see what the
//! launcher had put in place before the game started: `--report-file PATH` (the file's text, or null) and
//! `--report-reg "HKCU\Key|Name"` (a dword as a number, text as a string, or null). Both may repeat.
//!
//! `--launch-other NAME` makes it a launcher: it starts NAME (an exe next to it) with the same other arguments,
//! waits half a second and exits without a window, like a game launcher whose game owns the window.
//!
//! `--seat` falls back to the COUCHLINK_SEAT variable, `--out` to COUCHLINK_TESTGAME_OUT. Any other
//! argument is only recorded. `--hang-after-ms` stops answering window messages, to test a game that hangs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the process was started with.
struct Info {
    seat: Option<u32>,
    player: String,
    out: Option<PathBuf>,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    exit_after: Option<Duration>,
    hang_after: Option<Duration>,
    listen: Option<u16>,
    /// `"files":{...},"registry":{...}` as read at start.
    reported: String,
    started: Instant,
}

static INFO: OnceLock<Info> = OnceLock::new();

fn info() -> &'static Info {
    INFO.get().expect("info is set in main")
}

fn parse() -> Info {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env: BTreeMap<String, String> =
        std::env::vars().filter(|(k, _)| k.starts_with("COUCHLINK_") || k.starts_with("TESTGAME_")).collect();
    let mut seat = env.get("COUCHLINK_SEAT").and_then(|s| s.parse().ok());
    let mut player = env.get("COUCHLINK_PLAYER").cloned().unwrap_or_default();
    let mut out = env.get("COUCHLINK_TESTGAME_OUT").map(PathBuf::from);
    let (mut exit_after, mut hang_after, mut listen) = (None, None, None);
    let (mut report_files, mut report_regs): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--seat" => seat = value().parse().ok(),
            "--player" => player = value(),
            "--out" => out = Some(PathBuf::from(value())),
            "--exit-after-ms" => exit_after = value().parse().ok().map(Duration::from_millis),
            "--hang-after-ms" => hang_after = value().parse().ok().map(Duration::from_millis),
            "--listen" => listen = value().parse().ok(),
            "--report-file" => report_files.push(value()),
            "--report-reg" => report_regs.push(value()),
            _ => {}
        }
    }
    let reported = report(&report_files, &report_regs);
    Info { seat, player, out, args, env, exit_after, hang_after, listen, reported, started: Instant::now() }
}

/// The text of each file and the value of each registry entry, as JSON object members.
fn report(files: &[String], regs: &[String]) -> String {
    let files: Vec<String> = files
        .iter()
        .map(|f| format!("{}:{}", esc(f), std::fs::read_to_string(f).map_or("null".to_string(), |t| esc(&t))))
        .collect();
    let regs: Vec<String> = regs.iter().map(|r| format!("{}:{}", esc(r), registry_value(r))).collect();
    format!("\"files\":{{{}}},\"registry\":{{{}}}", files.join(","), regs.join(","))
}

/// `KEY|NAME` read with `reg query`: a dword as a number, anything else as text, a missing value as null.
fn registry_value(spec: &str) -> String {
    let Some((key, name)) = spec.split_once('|') else { return "null".to_string() };
    let Ok(out) = std::process::Command::new("reg").args(["query", key, "/v", name]).output() else { return "null".to_string() };
    if !out.status.success() {
        return "null".to_string();
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(name) else { continue };
        let mut parts = rest.trim_start().splitn(2, char::is_whitespace);
        let (kind, value) = (parts.next().unwrap_or(""), parts.next().unwrap_or("").trim());
        return match (kind, value.strip_prefix("0x")) {
            ("REG_DWORD", Some(hex)) => u32::from_str_radix(hex, 16).map_or("null".to_string(), |n| n.to_string()),
            _ => esc(value),
        };
    }
    "null".to_string()
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// What the window looks like right now.
#[derive(Default)]
struct WindowFacts {
    hwnd: usize,
    rect: [i32; 4],
    client: [i32; 2],
    style: u32,
    foreground: bool,
    visible: bool,
}

fn write_status(window: Option<&WindowFacts>, alive: bool) {
    let i = info();
    let Some(out) = &i.out else { return };
    let mut s = String::from("{");
    s.push_str(&format!("\"pid\":{},\"alive\":{alive}", std::process::id()));
    s.push_str(&format!(",\"seat\":{}", i.seat.map_or("null".to_string(), |n| n.to_string())));
    s.push_str(&format!(",\"player\":{}", esc(&i.player)));
    s.push_str(&format!(",\"uptime_ms\":{}", i.started.elapsed().as_millis()));
    s.push_str(&format!(",\"time_ms\":{}", SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())));
    s.push_str(&format!(",\"exe\":{}", esc(&std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default())));
    s.push_str(&format!(",\"cwd\":{}", esc(&std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default())));
    s.push_str(&format!(",\"listen\":{}", i.listen.map_or("null".to_string(), |p| p.to_string())));
    if let Some(w) = window {
        s.push_str(&format!(
            ",\"window\":{{\"hwnd\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"client_w\":{},\"client_h\":{},\"style\":{},\"visible\":{}}}",
            w.hwnd,
            w.rect[0],
            w.rect[1],
            w.rect[2] - w.rect[0],
            w.rect[3] - w.rect[1],
            w.client[0],
            w.client[1],
            w.style,
            w.visible
        ));
        s.push_str(&format!(",\"focused\":{}", w.foreground));
    } else {
        s.push_str(",\"window\":null,\"focused\":false");
    }
    s.push(',');
    s.push_str(&i.reported);
    s.push_str(",\"args\":[");
    s.push_str(&i.args.iter().map(|a| esc(a)).collect::<Vec<_>>().join(","));
    s.push_str("],\"env\":{");
    s.push_str(&i.env.iter().map(|(k, v)| format!("{}:{}", esc(k), esc(v))).collect::<Vec<_>>().join(","));
    s.push_str("}}");
    let tmp = out.with_extension("tmp");
    if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, out).is_err() {
        let _ = std::fs::remove_file(out);
        let _ = std::fs::rename(&tmp, out);
    }
}

// Like most real games, testgame imports version.dll. That import is what lets Couchlink's hook load as a stand-in
// `version.dll` next to this exe (no injection), so the session tests exercise the same path a game takes.
#[cfg(windows)]
#[link(name = "version", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
}

fn touch_version_dll() {
    #[cfg(windows)]
    {
        let path: Vec<u16> = r"C:\Windows\System32\kernel32.dll".encode_utf16().chain(std::iter::once(0)).collect();
        let mut handle = 0u32;
        // SAFETY: a NUL-terminated path and a valid out pointer.
        let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut handle) };
        std::hint::black_box(size);
    }
}

fn main() {
    touch_version_dll();
    INFO.set(parse()).ok().expect("set once");
    if let Some(other) = launch_other() {
        std::thread::sleep(Duration::from_millis(500));
        std::process::exit(if other { 0 } else { 1 });
    }
    if let Some(port) = info().listen {
        serve(port);
    } else {
        run_window();
    }
}

/// With `--launch-other NAME`: start NAME next to this exe with the remaining arguments. None without the flag.
fn launch_other() -> Option<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let at = args.iter().position(|a| a == "--launch-other")?;
    let name = args.get(at + 1)?;
    let rest: Vec<&String> = args.iter().enumerate().filter(|(i, _)| *i != at && *i != at + 1).map(|(_, a)| a).collect();
    let exe = std::env::current_exe().ok()?.with_file_name(name);
    Some(std::process::Command::new(exe).args(rest).spawn().is_ok())
}

/// A stand-in server: listens, accepts and drops connections, writes status, until told to stop.
fn serve(port: u16) {
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("testgame: cannot listen on {port}: {e}");
            std::process::exit(1);
        }
    };
    std::thread::spawn(move || for _ in listener.incoming() {});
    loop {
        write_status(None, true);
        if info().exit_after.is_some_and(|d| info().started.elapsed() >= d) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(not(windows))]
fn run_window() {
    eprintln!("testgame: the window mode only exists on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
fn run_window() {
    win::run();
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, FillRect, HBRUSH, HDC};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const TIMER_ID: usize = 1;
    const COLORS: [u32; 4] = [0x00FF_840A, 0x005B_6BFF, 0x0058_D130, 0x000A_D6FF]; // BGR of the seat colours

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn facts(hwnd: HWND) -> WindowFacts {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut c = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: plain queries on our own window with valid out pointers.
        unsafe {
            GetWindowRect(hwnd, &mut r);
            GetClientRect(hwnd, &mut c);
            WindowFacts {
                hwnd: hwnd as usize,
                rect: [r.left, r.top, r.right, r.bottom],
                client: [c.right - c.left, c.bottom - c.top],
                style: GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
                foreground: GetForegroundWindow() == hwnd,
                visible: IsWindowVisible(hwnd) != 0,
            }
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: called by Windows on our own thread with a valid window.
        unsafe {
            match msg {
                WM_TIMER => {
                    let i = info();
                    if i.hang_after.is_some_and(|d| i.started.elapsed() >= d) {
                        loop {
                            std::thread::sleep(Duration::from_secs(3600));
                        }
                    }
                    if i.exit_after.is_some_and(|d| i.started.elapsed() >= d) {
                        PostQuitMessage(0);
                        return 0;
                    }
                    write_status(Some(&facts(hwnd)), true);
                    0
                }
                WM_ERASEBKGND => {
                    let seat = info().seat.unwrap_or(0) as usize % COLORS.len();
                    let brush: HBRUSH = CreateSolidBrush(COLORS[seat]);
                    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetClientRect(hwnd, &mut r);
                    FillRect(wparam as HDC, &r, brush);
                    windows_sys::Win32::Graphics::Gdi::DeleteObject(brush as _);
                    1
                }
                WM_DESTROY => {
                    write_status(Some(&facts(hwnd)), false);
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    pub fn run() {
        let i = info();
        // SAFETY: standard window class registration, creation and message loop, all on this thread.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hinstance = GetModuleHandleW(std::ptr::null());
            let class = wide("CouchlinkTestGame");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            if RegisterClassExW(&wc) == 0 {
                eprintln!("testgame: cannot register the window class");
                std::process::exit(1);
            }
            let title = wide(&format!("Couchlink testgame seat {}", i.seat.map_or("-".to_string(), |s| s.to_string())));
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                200,
                150,
                640,
                400,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                eprintln!("testgame: cannot create the window");
                std::process::exit(1);
            }
            ShowWindow(hwnd, SW_SHOW);
            SetTimer(hwnd, TIMER_ID, 200, None);
            write_status(Some(&facts(hwnd)), true);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            write_status(None, false);
        }
    }
}
