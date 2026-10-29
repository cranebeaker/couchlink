//! Display detection.
//!
//! On Windows monitors are read with Win32 on their own thread, never through
//! SDL video: `SDL_InitSubSystem(VIDEO)` hangs while the lock screen is up, and
//! display detection must never be able to block gamepads, the WebSocket server
//! or daemon.json. On Linux and macOS SDL video is used (see `sdl.rs`), but the
//! shared bookkeeping here (stable ids, change detection) is used by both.

use couchlink_core::Display;

/// One monitor as reported by the OS, before ids are assigned.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDisplay {
    /// OS device name (for example `\\.\DISPLAY1`). Ids are keyed by this.
    pub device: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub hz: f32,
    pub primary: bool,
}

/// Stable display id for an OS device name: `\\.\DISPLAY1` becomes `disp:DISPLAY1`.
pub fn display_id(device: &str) -> String {
    let trimmed = device.trim().trim_start_matches(r"\\.\").trim_start_matches(r"\\?\");
    format!("disp:{trimmed}")
}

/// Turn raw monitors into protocol displays: sorted by device name (so poll order
/// does not matter), with unique ids even if the OS repeats a device name.
pub fn build_list(mut raw: Vec<RawDisplay>) -> Vec<Display> {
    raw.sort_by(|a, b| (a.device.len(), &a.device).cmp(&(b.device.len(), &b.device)));
    let mut out: Vec<Display> = Vec::with_capacity(raw.len());
    for r in raw {
        let base = display_id(&r.device);
        let mut id = base.clone();
        let mut n = 2;
        while out.iter().any(|d| d.id == id) {
            id = format!("{base}#{n}");
            n += 1;
        }
        out.push(Display {
            id,
            name: r.name,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            scale: r.scale,
            hz: r.hz,
            primary: r.primary,
            used: false,
        });
    }
    out
}

/// True when the only monitor is Windows' `WinDisc` stand-in (see `Tracker::update`).
fn is_placeholder_only(raw: &[RawDisplay]) -> bool {
    !raw.is_empty() && raw.iter().all(|r| display_id(&r.device).eq_ignore_ascii_case("disp:WinDisc"))
}

/// Remembers the last list and reports only real changes.
#[derive(Default)]
pub struct Tracker {
    last: Option<Vec<Display>>,
}

impl Tracker {
    /// The new list if it differs from the previous one (the first poll always counts).
    pub fn update(&mut self, raw: Vec<RawDisplay>) -> Option<Vec<Display>> {
        // While the lock screen (or a disconnected session) owns the input desktop,
        // Windows shows one fake monitor called WinDisc. Keep the last real list
        // through that, and report nothing if there never was one.
        let raw = if is_placeholder_only(&raw) {
            if self.last.is_some() {
                return None;
            }
            Vec::new()
        } else {
            raw
        };
        let list = build_list(raw);
        if self.last.as_ref() == Some(&list) {
            return None;
        }
        self.last = Some(list.clone());
        Some(list)
    }
}

#[cfg(windows)]
pub use win::{spawn, DisplayThread};

#[cfg(windows)]
mod win {
    use super::{RawDisplay, Tracker};
    use crate::sdl::SdlEvent;
    use std::sync::mpsc as std_mpsc;
    use std::thread::JoinHandle;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedSender;
    use windows_sys::Win32::Devices::Display::*;
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::HiDpi::*;
    use windows_sys::core::BOOL;

    const POLL: Duration = Duration::from_secs(2);
    const MONITORINFOF_PRIMARY: u32 = 1;

    /// Handle to the display thread. Dropping it stops the thread.
    pub struct DisplayThread {
        _stop: std_mpsc::Sender<()>,
        _thread: JoinHandle<()>,
    }

    /// Start the display thread. Returns at once; it never waits on Win32.
    pub fn spawn(events: UnboundedSender<SdlEvent>) -> Result<DisplayThread, String> {
        let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("displays".into())
            .spawn(move || run(events, stop_rx))
            .map_err(|e| format!("cannot start display thread: {e}"))?;
        Ok(DisplayThread { _stop: stop_tx, _thread: thread })
    }

    fn run(events: UnboundedSender<SdlEvent>, stop: std_mpsc::Receiver<()>) {
        let trace = std::env::var_os("COUCHLINK_TRACE").is_some();
        let step = |what: &str| {
            if trace {
                eprintln!("couchlinkd: displays {what}")
            }
        };
        step("thread start");
        // Per-monitor v2 makes every size below physical pixels, as the protocol says.
        // SAFETY: plain FFI call; affects only this thread.
        let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        step(if prev.is_null() { "dpi awareness failed" } else { "dpi aware" });
        let mut tracker = Tracker::default();
        loop {
            step("poll");
            let raw = read_monitors();
            step(&format!("read {} monitors", raw.len()));
            if let Some(list) = tracker.update(raw) {
                step(&format!("changed, {} displays", list.len()));
                if events.send(SdlEvent::Displays(list)).is_err() {
                    return;
                }
            }
            match stop.recv_timeout(POLL) {
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                _ => return,
            }
        }
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end]).trim().to_string()
    }

    unsafe extern "system" fn collect(h: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed by `read_monitors`.
        let v = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
        v.push(h);
        1
    }

    /// Current monitors. Any failure yields fewer (or no) entries, never a panic or a wait.
    fn read_monitors() -> Vec<RawDisplay> {
        let mut handles: Vec<HMONITOR> = Vec::new();
        // SAFETY: the callback only pushes into `handles`, which outlives the call.
        unsafe {
            EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut handles as *mut _ as LPARAM);
        }
        if handles.is_empty() {
            return Vec::new();
        }
        let friendly = friendly_names();
        let mut out = Vec::new();
        for h in handles {
            // SAFETY: zeroed MONITORINFOEXW with cbSize set is the documented way to call this.
            let mut mi: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) } == 0 {
                continue;
            }
            let device = from_wide(&mi.szDevice);
            let r = mi.monitorInfo.rcMonitor;

            let (mut dx, mut dy) = (0u32, 0u32);
            // SAFETY: out pointers are valid locals.
            let hr = unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            let scale = if hr >= 0 && dx > 0 { dx as f32 / 96.0 } else { 1.0 };

            let hz = refresh_rate(&mi.szDevice);
            let name = friendly
                .iter()
                .find(|(dev, _)| dev.eq_ignore_ascii_case(&device))
                .map(|(_, n)| n.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| device_string(&mi.szDevice))
                .unwrap_or_else(|| device.trim_start_matches(r"\\.\").to_string());

            out.push(RawDisplay {
                device,
                name,
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
                scale,
                hz,
                primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        out
    }

    fn refresh_rate(device: &[u16; 32]) -> f32 {
        // SAFETY: zeroed DEVMODEW with dmSize set; `device` is NUL terminated.
        unsafe {
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 && dm.dmDisplayFrequency > 1 {
                dm.dmDisplayFrequency as f32
            } else {
                0.0
            }
        }
    }

    /// The device string of the monitor attached to a GDI device, for example
    /// "Generic PnP Monitor". Used when no friendly name is available.
    fn device_string(device: &[u16; 32]) -> Option<String> {
        // SAFETY: zeroed DISPLAY_DEVICEW with cb set; `device` is NUL terminated.
        unsafe {
            let mut dd: DISPLAY_DEVICEW = std::mem::zeroed();
            dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(device.as_ptr(), 0, &mut dd, 0) == 0 {
                return None;
            }
            let s = from_wide(&dd.DeviceString);
            if s.is_empty() { None } else { Some(s) }
        }
    }

    /// GDI device name to friendly monitor name, from the display config API.
    fn friendly_names() -> Vec<(String, String)> {
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
        let mut ok = false;
        for _ in 0..4 {
            let (mut np, mut nm) = (0u32, 0u32);
            // SAFETY: out pointers are valid locals.
            if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) } != ERROR_SUCCESS {
                return Vec::new();
            }
            // SAFETY: zeroed plain-data structs.
            paths = (0..np).map(|_| unsafe { std::mem::zeroed() }).collect();
            let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = (0..nm).map(|_| unsafe { std::mem::zeroed() }).collect();
            // SAFETY: buffers hold np and nm elements.
            let rc = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut np,
                    paths.as_mut_ptr(),
                    &mut nm,
                    modes.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            if rc == ERROR_SUCCESS {
                paths.truncate(np as usize);
                ok = true;
                break;
            }
            if rc != ERROR_INSUFFICIENT_BUFFER {
                return Vec::new();
            }
        }
        if !ok {
            return Vec::new();
        }
        let mut out = Vec::new();
        for p in &paths {
            // SAFETY: zeroed request packets with header type, size, adapter and id set.
            unsafe {
                let mut src: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
                src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                src.header.adapterId = p.sourceInfo.adapterId;
                src.header.id = p.sourceInfo.id;
                if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                    continue;
                }
                let mut tgt: DISPLAYCONFIG_TARGET_DEVICE_NAME = std::mem::zeroed();
                tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
                tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
                tgt.header.adapterId = p.targetInfo.adapterId;
                tgt.header.id = p.targetInfo.id;
                if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                    continue;
                }
                out.push((from_wide(&src.viewGdiDeviceName), from_wide(&tgt.monitorFriendlyDeviceName)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(device: &str, x: i32, primary: bool) -> RawDisplay {
        RawDisplay {
            device: device.into(),
            name: format!("Monitor {device}"),
            x,
            y: 0,
            w: 2560,
            h: 1440,
            scale: 1.0,
            hz: 60.0,
            primary,
        }
    }

    #[test]
    fn ids_come_from_device_names() {
        assert_eq!(display_id(r"\\.\DISPLAY1"), "disp:DISPLAY1");
        assert_eq!(display_id("DISPLAY2"), "disp:DISPLAY2");
    }

    #[test]
    fn order_of_polling_does_not_matter() {
        let a = build_list(vec![raw(r"\\.\DISPLAY2", 2560, false), raw(r"\\.\DISPLAY1", 0, true)]);
        let b = build_list(vec![raw(r"\\.\DISPLAY1", 0, true), raw(r"\\.\DISPLAY2", 2560, false)]);
        assert_eq!(a, b);
        assert_eq!(a[0].id, "disp:DISPLAY1");
        assert_eq!(a[1].id, "disp:DISPLAY2");
    }

    #[test]
    fn natural_order_for_ten_or_more() {
        let l = build_list(vec![raw(r"\\.\DISPLAY10", 0, false), raw(r"\\.\DISPLAY2", 0, false)]);
        assert_eq!(l[0].id, "disp:DISPLAY2");
    }

    #[test]
    fn repeated_device_names_get_unique_ids() {
        let l = build_list(vec![raw(r"\\.\DISPLAY1", 0, true), raw(r"\\.\DISPLAY1", 100, false)]);
        assert_eq!(l[0].id, "disp:DISPLAY1");
        assert_eq!(l[1].id, "disp:DISPLAY1#2");
    }

    #[test]
    fn tracker_reports_only_changes() {
        let mut t = Tracker::default();
        let one = vec![raw(r"\\.\DISPLAY1", 0, true)];
        assert!(t.update(one.clone()).is_some(), "first poll always reports");
        assert!(t.update(one.clone()).is_none(), "same list reports nothing");

        let mut two = one.clone();
        two.push(raw(r"\\.\DISPLAY2", 2560, false));
        let l = t.update(two.clone()).expect("added monitor reports");
        assert_eq!(l.len(), 2);
        assert!(t.update(two.clone()).is_none());

        let mut moved = two.clone();
        moved[1].x = -2560;
        assert!(t.update(moved).is_some(), "moved monitor reports");
    }

    #[test]
    fn ids_stay_stable_when_other_monitors_come_and_go() {
        let mut t = Tracker::default();
        let first = t.update(vec![raw(r"\\.\DISPLAY1", 0, true), raw(r"\\.\DISPLAY2", 2560, false)]).unwrap();
        let after = t.update(vec![raw(r"\\.\DISPLAY2", 0, true)]).unwrap();
        assert_eq!(first[1].id, after[0].id);
        let back = t.update(vec![raw(r"\\.\DISPLAY2", 2560, false), raw(r"\\.\DISPLAY1", 0, true)]).unwrap();
        assert_eq!(back, first);
    }

    #[test]
    fn windisc_placeholder_keeps_the_last_real_list() {
        let mut t = Tracker::default();
        let real = vec![raw(r"\\.\DISPLAY1", 0, true)];
        let placeholder = vec![raw("WinDisc", 0, true)];
        let first = t.update(real.clone()).unwrap();
        assert!(t.update(placeholder.clone()).is_none(), "lock screen placeholder is ignored");
        assert!(t.update(real).is_none(), "unlock changes nothing");
        assert_eq!(first.len(), 1);
        assert_eq!(Tracker::default().update(placeholder), Some(Vec::new()), "never a real list: empty");
    }

    #[test]
    fn empty_list_is_reported_once_then_recovers() {
        let mut t = Tracker::default();
        assert_eq!(t.update(Vec::new()), Some(Vec::new()));
        assert!(t.update(Vec::new()).is_none());
        assert_eq!(t.update(vec![raw(r"\\.\DISPLAY1", 0, true)]).unwrap().len(), 1);
    }
}
