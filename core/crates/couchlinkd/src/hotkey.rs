//! The close-the-games keys, system-wide: they work while a game has the focus. On Windows a thread of its own
//! registers them with `RegisterHotKey` and turns each press into `HubMsg::StopShortcut`. Elsewhere this does
//! nothing yet (Linux reads its keyboards in `session/rawkbm_linux.rs`).

use crate::hub::HubMsg;
use couchlink_core::shortcut::KeyCombo;
use tokio::sync::mpsc::UnboundedSender;

pub struct Hotkey {
    #[cfg(windows)]
    win: win::Thread,
}

impl Hotkey {
    /// Use these keys from now on (none: no key shortcut).
    pub fn set(&self, keys: Option<KeyCombo>) -> Result<(), String> {
        #[cfg(windows)]
        return self.win.set(keys);
        #[cfg(not(windows))]
        {
            let _ = keys;
            Ok(())
        }
    }
}

pub fn spawn(tx: UnboundedSender<HubMsg>) -> Hotkey {
    #[cfg(windows)]
    return Hotkey { win: win::Thread::spawn(tx) };
    #[cfg(not(windows))]
    {
        let _ = tx;
        Hotkey {}
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::mpsc;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PeekMessageW, PostThreadMessageW, PM_NOREMOVE, WM_APP, WM_HOTKEY};

    const ID: i32 = 1;
    const WM_SET: u32 = WM_APP + 1;

    pub struct Thread {
        id: u32,
        keys: mpsc::Sender<(Option<KeyCombo>, mpsc::Sender<Result<(), String>>)>,
    }

    impl Thread {
        pub fn spawn(tx: UnboundedSender<HubMsg>) -> Thread {
            let (keys_tx, keys_rx) = mpsc::channel::<(Option<KeyCombo>, mpsc::Sender<Result<(), String>>)>();
            let (id_tx, id_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("hotkey".into())
                .spawn(move || {
                    let mut msg: MSG = unsafe { std::mem::zeroed() };
                    // SAFETY: plain Win32 calls on this thread's own message queue; `msg` is a valid out buffer.
                    unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) }; // makes the queue
                    let _ = id_tx.send(unsafe { GetCurrentThreadId() });
                    let mut registered = false;
                    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                        match msg.message {
                            WM_HOTKEY if msg.wParam == ID as usize => {
                                let _ = tx.send(HubMsg::StopShortcut);
                            }
                            WM_SET => {
                                while let Ok((keys, reply)) = keys_rx.try_recv() {
                                    if registered {
                                        unsafe { UnregisterHotKey(std::ptr::null_mut(), ID) };
                                        registered = false;
                                    }
                                    let result = match keys {
                                        None => Ok(()),
                                        Some(k) => {
                                            let mut mods = MOD_NOREPEAT;
                                            for (on, m) in [(k.ctrl, MOD_CONTROL), (k.shift, MOD_SHIFT), (k.alt, MOD_ALT), (k.win, MOD_WIN)] {
                                                if on {
                                                    mods |= m;
                                                }
                                            }
                                            if unsafe { RegisterHotKey(std::ptr::null_mut(), ID, mods, k.virtual_key() as u32) } != 0 {
                                                registered = true;
                                                Ok(())
                                            } else {
                                                Err(format!("{} is taken", k.label()))
                                            }
                                        }
                                    };
                                    let _ = reply.send(result);
                                }
                            }
                            _ => {}
                        }
                    }
                })
                .expect("hotkey thread");
            Thread { id: id_rx.recv().unwrap_or(0), keys: keys_tx }
        }

        pub fn set(&self, keys: Option<KeyCombo>) -> Result<(), String> {
            let (reply_tx, reply_rx) = mpsc::channel();
            self.keys.send((keys, reply_tx)).map_err(|_| "the hotkey thread stopped".to_string())?;
            // SAFETY: posting a message to a thread id this module created.
            unsafe { PostThreadMessageW(self.id, WM_SET, 0, 0) };
            reply_rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "the hotkey thread did not answer".to_string())?
        }
    }
}
