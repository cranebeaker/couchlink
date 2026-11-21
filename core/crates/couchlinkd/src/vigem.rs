//! A virtual Xbox 360 controller on this PC (Windows), through the ViGEmBus driver. "Send to Windows" gives a player's
//! controllers to Windows as one merged controller, so any game, emulator or program sees a single pad that plays
//! what all of them do.
//!
//! The driver is a separate download (ViGEmBus by Nefarius); without it [`available`] is false and nothing is sent.
//! The bus is spoken to directly (its IOCTLs), so Couchlink ships nothing of the driver's.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::session::padfeed::PadState;

/// True when the ViGEmBus driver is installed and running.
pub fn available() -> bool {
    #[cfg(windows)]
    return win::open_bus().is_ok();
    #[cfg(not(windows))]
    false
}

/// One virtual controller; unplugged when dropped.
pub struct Target {
    #[cfg(windows)]
    inner: win::Inner,
}

impl Target {
    /// Plugs a new virtual Xbox 360 controller in.
    pub fn plug() -> Result<Target, String> {
        #[cfg(windows)]
        return win::Inner::plug().map(|inner| Target { inner });
        #[cfg(not(windows))]
        Err("virtual controllers are made on Windows only".to_string())
    }

    /// Sets what the controller reports.
    pub fn submit(&self, state: &PadState) {
        #[cfg(windows)]
        self.inner.submit(state);
        #[cfg(not(windows))]
        let _ = state;
    }

    /// Rumble the game or program asked of the controller since the last call: (left, right) motors, 0..65535.
    pub fn take_rumble(&self) -> Option<(u16, u16)> {
        #[cfg(windows)]
        return self.inner.take_rumble();
        #[cfg(not(windows))]
        None
    }
}

#[cfg(windows)]
mod win {
    use super::PadState;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
    use std::sync::Arc;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows_sys::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};

    /// The bus driver's device interface.
    const BUS_GUID: GUID = GUID { data1: 0x96E4_2B22, data2: 0xF5E9, data3: 0x42F8, data4: [0xB0, 0x43, 0xED, 0x0F, 0x93, 0x2F, 0x01, 0x4F] };

    // CTL_CODE(FILE_DEVICE_BUS_EXTENDER = 0x2A, 0x801 + n, METHOD_BUFFERED, FILE_WRITE_DATA) and the read/write one.
    const IOCTL_PLUGIN_TARGET: u32 = 0x002A_A004;
    const IOCTL_UNPLUG_TARGET: u32 = 0x002A_A008;
    const IOCTL_CHECK_VERSION: u32 = 0x002A_A00C;
    const IOCTL_WAIT_DEVICE_READY: u32 = 0x002A_A010;
    const IOCTL_XUSB_SUBMIT_REPORT: u32 = 0x002A_A808;
    const IOCTL_XUSB_REQUEST_NOTIFICATION: u32 = 0x002A_E800;
    const BUS_VERSION: u32 = 0x0001;

    #[repr(C)]
    struct CheckVersion {
        size: u32,
        version: u32,
    }
    #[repr(C)]
    struct PlugIn {
        size: u32,
        serial: u32,
        target_type: u32,
        vendor: u16,
        product: u16,
    }
    #[repr(C)]
    struct Serial {
        size: u32,
        serial: u32,
    }
    #[repr(C)]
    struct Submit {
        size: u32,
        serial: u32,
        buttons: u16,
        lt: u8,
        rt: u8,
        lx: i16,
        ly: i16,
        rx: i16,
        ry: i16,
    }
    /// XUSB_REQUEST_NOTIFICATION: Size, SerialNo, then the motors and the pad's LED number.
    #[repr(C)]
    struct Notification {
        size: u32,
        serial: u32,
        large: u8,
        small: u8,
        led: u8,
    }

    pub struct Bus(HANDLE);
    // SAFETY: a handle opened for overlapped I/O; every call carries its own OVERLAPPED.
    unsafe impl Send for Bus {}
    unsafe impl Sync for Bus {}

    impl Drop for Bus {
        fn drop(&mut self) {
            // SAFETY: the handle is ours.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Opens the bus driver, when it is there.
    pub fn open_bus() -> Result<Bus, String> {
        // SAFETY: the usual SetupAPI walk; every buffer is as long as the length passed; the list is destroyed.
        unsafe {
            let set = SetupDiGetClassDevsW(&BUS_GUID, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
            if set as isize == -1 {
                return Err("the ViGEmBus driver is not installed".to_string());
            }
            let mut data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &BUS_GUID, 0, &mut data) == 0 {
                SetupDiDestroyDeviceInfoList(set);
                return Err("the ViGEmBus driver is not installed".to_string());
            }
            let mut need = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &data, std::ptr::null_mut(), 0, &mut need, std::ptr::null_mut());
            let mut buf = vec![0u8; need.max(8) as usize];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = if cfg!(target_pointer_width = "64") { 8 } else { 6 };
            let ok = SetupDiGetDeviceInterfaceDetailW(set, &data, detail, need, std::ptr::null_mut(), std::ptr::null_mut());
            SetupDiDestroyDeviceInfoList(set);
            if ok == 0 {
                return Err("the ViGEmBus driver cannot be read".to_string());
            }
            let path = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let h = CreateFileW(path, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED, std::ptr::null_mut());
            if h == INVALID_HANDLE_VALUE {
                return Err(format!("the ViGEmBus driver cannot be opened (error {})", GetLastError()));
            }
            let bus = Bus(h);
            let check = CheckVersion { size: std::mem::size_of::<CheckVersion>() as u32, version: BUS_VERSION };
            bus.ioctl(IOCTL_CHECK_VERSION, &check as *const _ as *const c_void, check.size, std::ptr::null_mut(), 0, 5000).map_err(|e| format!("the ViGEmBus driver is a version Couchlink does not know ({e})"))?;
            Ok(bus)
        }
    }

    impl Bus {
        /// One overlapped IOCTL, waited for. `Err` is the Windows error.
        unsafe fn ioctl(&self, code: u32, input: *const c_void, in_len: u32, output: *mut c_void, out_len: u32, wait_ms: u32) -> Result<u32, u32> {
            // SAFETY: the buffers are the caller's, valid for the call; the event and OVERLAPPED live until the call is done.
            unsafe {
                let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = event;
                let mut got = 0u32;
                let ok = DeviceIoControl(self.0, code, input, in_len, output, out_len, &mut got, &mut ov);
                let result = if ok != 0 {
                    Ok(got)
                } else if GetLastError() == ERROR_IO_PENDING {
                    if WaitForSingleObject(event, wait_ms) == 0 && GetOverlappedResult(self.0, &ov, &mut got, 0) != 0 {
                        Ok(got)
                    } else {
                        Err(GetLastError())
                    }
                } else {
                    Err(GetLastError())
                };
                CloseHandle(event);
                result
            }
        }
    }

    pub struct Inner {
        bus: Arc<Bus>,
        serial: u32,
        rumble: Arc<(AtomicU16, AtomicBool)>,
        stop: Arc<AtomicBool>,
    }

    static NEXT_SERIAL: AtomicU32 = AtomicU32::new(0x4C00);

    impl Inner {
        pub fn plug() -> Result<Inner, String> {
            let bus = Arc::new(open_bus()?);
            // A serial number no other client uses: Couchlink's own range.
            for _ in 0..64 {
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::SeqCst);
                let plug = PlugIn { size: std::mem::size_of::<PlugIn>() as u32, serial, target_type: 0, vendor: 0x045E, product: 0x028E };
                // SAFETY: a struct of the size passed; no output.
                match unsafe { bus.ioctl(IOCTL_PLUGIN_TARGET, &plug as *const _ as *const c_void, plug.size, std::ptr::null_mut(), 0, 5000) } {
                    Ok(_) => {
                        let ready = Serial { size: std::mem::size_of::<Serial>() as u32, serial };
                        // The driver finishes making the device in the background; this waits for it.
                        // SAFETY: as above.
                        let _ = unsafe { bus.ioctl(IOCTL_WAIT_DEVICE_READY, &ready as *const _ as *const c_void, ready.size, std::ptr::null_mut(), 0, 10_000) };
                        let rumble = Arc::new((AtomicU16::new(0), AtomicBool::new(false)));
                        let stop = Arc::new(AtomicBool::new(false));
                        let inner = Inner { bus: bus.clone(), serial, rumble: rumble.clone(), stop: stop.clone() };
                        // Rumble: one request waits at the driver for the game's next motor setting.
                        std::thread::Builder::new()
                            .name("vigem-rumble".into())
                            .spawn(move || {
                                while !stop.load(Ordering::Relaxed) {
                                    let mut n = Notification { size: std::mem::size_of::<Notification>() as u32, serial, large: 0, small: 0, led: 0 };
                                    let ptr = &mut n as *mut Notification as *mut c_void;
                                    // SAFETY: input and output are the same struct, of the size passed.
                                    let r = unsafe { bus.ioctl(IOCTL_XUSB_REQUEST_NOTIFICATION, ptr, n.size, ptr, n.size, 500) };
                                    if r.is_ok() {
                                        let motors = ((n.large as u16) << 8) | n.small as u16;
                                        rumble.0.store(motors, Ordering::SeqCst);
                                        rumble.1.store(true, Ordering::SeqCst);
                                    } else {
                                        std::thread::sleep(std::time::Duration::from_millis(50));
                                    }
                                }
                            })
                            .ok();
                        return Ok(inner);
                    }
                    Err(_) => continue,
                }
            }
            Err("the ViGEmBus driver would not make a controller".to_string())
        }

        pub fn submit(&self, s: &PadState) {
            let report = Submit { size: std::mem::size_of::<Submit>() as u32, serial: self.serial, buttons: s.buttons, lt: s.lt, rt: s.rt, lx: s.lx, ly: s.ly, rx: s.rx, ry: s.ry };
            // SAFETY: a struct of the size passed; no output.
            let _ = unsafe { self.bus.ioctl(IOCTL_XUSB_SUBMIT_REPORT, &report as *const _ as *const c_void, report.size, std::ptr::null_mut(), 0, 1000) };
        }

        pub fn take_rumble(&self) -> Option<(u16, u16)> {
            if self.rumble.1.swap(false, Ordering::SeqCst) {
                let m = self.rumble.0.load(Ordering::SeqCst);
                // The report carries 0..255 per motor; the pads take 0..65535.
                Some((((m >> 8) as u16) * 257, ((m & 0xFF) as u16) * 257))
            } else {
                None
            }
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let unplug = Serial { size: std::mem::size_of::<Serial>() as u32, serial: self.serial };
            // SAFETY: a struct of the size passed; no output.
            let _ = unsafe { self.bus.ioctl(IOCTL_UNPLUG_TARGET, &unplug as *const _ as *const c_void, unplug.size, std::ptr::null_mut(), 0, 5000) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use couchlink_hook::feed::buttons as xb;
    use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

    /// The connected XInput slots and what each reports.
    fn slots() -> Vec<(u32, XINPUT_STATE)> {
        (0..4u32)
            .filter_map(|i| {
                // SAFETY: an out struct on the stack.
                let mut s: XINPUT_STATE = unsafe { std::mem::zeroed() };
                (unsafe { XInputGetState(i, &mut s) } == 0).then_some((i, s))
            })
            .collect()
    }

    /// With the driver installed, a virtual controller shows up in XInput and reports what is sent; without it the test
    /// has nothing to check and says so.
    #[test]
    fn a_virtual_controller_reports_what_is_sent() {
        if !available() {
            println!("SKIP (the ViGEmBus driver is not installed)");
            return;
        }
        let before: Vec<u32> = slots().into_iter().map(|(i, _)| i).collect();
        let pad = Target::plug().expect("plug a virtual controller in");
        let state = PadState { buttons: xb::A | xb::DPAD_UP, lt: 200, rt: 17, lx: 12000, ly: -9000, rx: -3000, ry: 4000 };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = None;
        while seen.is_none() && std::time::Instant::now() < end {
            pad.submit(&state);
            std::thread::sleep(std::time::Duration::from_millis(100));
            seen = slots().into_iter().find(|(i, s)| !before.contains(i) && s.Gamepad.wButtons == state.buttons);
        }
        let (slot, s) = seen.expect("the new controller shows in XInput with the buttons sent");
        println!("virtual controller is XInput slot {slot}");
        assert_eq!(s.Gamepad.bLeftTrigger, 200);
        assert_eq!(s.Gamepad.bRightTrigger, 17);
        assert_eq!((s.Gamepad.sThumbLX, s.Gamepad.sThumbLY), (12000, -9000));
        assert_eq!((s.Gamepad.sThumbRX, s.Gamepad.sThumbRY), (-3000, 4000));
        drop(pad);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after: Vec<u32> = slots().into_iter().map(|(i, _)| i).collect();
        assert_eq!(after, before, "unplugged again when dropped");
    }
}
