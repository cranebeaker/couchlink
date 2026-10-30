//! Virtual input devices made through /dev/uinput, for tests: the daemon's unit tests and the integration test
//! (which includes this file by path) use it to make gamepads, keyboards and mice nobody has to plug in.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const BTN_SOUTH: u16 = 0x130;
pub const BTN_EAST: u16 = 0x131;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

/// A virtual device with the given keys (and relative axes, for a mouse); gone when dropped.
pub struct VirtualDevice {
    file: File,
    pub name: String,
}

impl VirtualDevice {
    pub fn new(name: &str, keys: &[u16], rels: &[u16]) -> Result<VirtualDevice, String> {
        let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput").map_err(|e| format!("/dev/uinput: {e}"))?;
        let fd = file.as_raw_fd();
        let mut setup = UinputSetup { bustype: 0x06, vendor: 0x1209, product: 0x0001, version: 1, name: [0; 80], ff_effects_max: 0 };
        setup.name[..name.len()].copy_from_slice(name.as_bytes());
        // SAFETY: uinput ioctls on our own descriptor with the argument types the kernel expects.
        let ok = unsafe {
            libc::ioctl(fd, UI_SET_EVBIT as _, EV_KEY as libc::c_int) == 0
                && keys.iter().all(|k| libc::ioctl(fd, UI_SET_KEYBIT as _, *k as libc::c_int) == 0)
                && (rels.is_empty() || libc::ioctl(fd, UI_SET_EVBIT as _, EV_REL as libc::c_int) == 0)
                && rels.iter().all(|r| libc::ioctl(fd, UI_SET_RELBIT as _, *r as libc::c_int) == 0)
                && libc::ioctl(fd, UI_DEV_SETUP as _, &setup as *const UinputSetup) == 0
                && libc::ioctl(fd, UI_DEV_CREATE as _) == 0
        };
        if !ok {
            return Err(format!("uinput setup failed: {}", std::io::Error::last_os_error()));
        }
        Ok(VirtualDevice { file, name: name.to_string() })
    }

    /// The device's event node, once udev has made it readable.
    pub fn node(&self, within: Duration) -> Result<PathBuf, String> {
        let end = Instant::now() + within;
        loop {
            for e in std::fs::read_dir("/sys/class/input").map_err(|e| e.to_string())?.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let named = std::fs::read_to_string(e.path().join("device/name")).is_ok_and(|s| s.trim() == self.name);
                if n.strip_prefix("event").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) && named {
                    let node = Path::new("/dev/input").join(&n);
                    if File::open(&node).is_ok() {
                        return Ok(node);
                    }
                }
            }
            if Instant::now() > end {
                return Err(format!("no readable event node for {} (is there a udev rule making it readable?)", self.name));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn emit(&mut self, kind: u16, code: u16, value: i32) {
        let mut ev = [0u8; 24];
        ev[16..18].copy_from_slice(&kind.to_ne_bytes());
        ev[18..20].copy_from_slice(&code.to_ne_bytes());
        ev[20..24].copy_from_slice(&value.to_ne_bytes());
        self.file.write_all(&ev).expect("write to uinput");
    }

    pub fn press(&mut self, key: u16) {
        for v in [1, 0] {
            self.emit(EV_KEY, key, v);
            self.emit(EV_SYN, 0, 0);
        }
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        // SAFETY: destroying the device made on this descriptor.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}

/// The machine can make virtual devices, or why not.
pub fn skip_reason() -> Option<String> {
    OpenOptions::new().write(true).open("/dev/uinput").err().map(|e| format!("/dev/uinput: {e}"))
}
