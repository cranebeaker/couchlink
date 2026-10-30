//! The shortcut keys (close the games, bring them to the front, hide them), system-wide: they work while a game has the focus. On Windows a thread of its own
//! registers them with `RegisterHotKey` and turns each press into `HubMsg::StopShortcut`. On Linux the keyboard
//! watcher (`session/rawkbm_linux.rs`), which reads every keyboard, seated or not, reports each key press with the
//! modifiers held on that keyboard ([`linux::key_down`]). macOS does nothing yet.

use crate::hub::HubMsg;
use couchlink_core::shortcut::KeyCombo;
use tokio::sync::mpsc::UnboundedSender;

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Close the games and put the desktop back.
    Stop,
    /// Bring every game window to the front.
    Front,
    /// Hide the game windows (back to the desktop).
    Hide,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Stop, Action::Front, Action::Hide];

    fn index(self) -> usize {
        self as usize
    }

    /// What the hub hears when this shortcut is pressed.
    pub fn msg(self) -> HubMsg {
        match self {
            Action::Stop => HubMsg::StopShortcut,
            Action::Front => HubMsg::WindowsShortcut(true),
            Action::Hide => HubMsg::WindowsShortcut(false),
        }
    }
}

pub struct Hotkey {
    #[cfg(windows)]
    win: win::Thread,
    #[cfg(target_os = "linux")]
    tx: UnboundedSender<HubMsg>,
}

impl Hotkey {
    /// Use these keys for `action` from now on (none: no key for it).
    pub fn set(&self, action: Action, keys: Option<KeyCombo>) -> Result<(), String> {
        #[cfg(windows)]
        return self.win.set(action, keys);
        #[cfg(target_os = "linux")]
        return linux::set(action, keys, &self.tx);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = (action, keys);
            Ok(())
        }
    }
}

pub fn spawn(tx: UnboundedSender<HubMsg>) -> Hotkey {
    #[cfg(windows)]
    return Hotkey { win: win::Thread::spawn(tx) };
    #[cfg(target_os = "linux")]
    return Hotkey { tx };
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = tx;
        Hotkey {}
    }
}

#[cfg(target_os = "linux")]
pub mod linux {
    use super::*;
    use std::sync::Mutex;

    /// Modifier bits, as the keyboard watcher keeps them for each keyboard.
    pub const CTRL: u8 = 1;
    pub const SHIFT: u8 = 2;
    pub const ALT: u8 = 4;
    pub const WIN: u8 = 8;

    /// The modifier bit of an evdev key code (left or right), or 0.
    pub fn modifier(code: u16) -> u8 {
        match code {
            29 | 97 => CTRL,
            42 | 54 => SHIFT,
            56 | 100 => ALT,
            125 | 126 => WIN,
            _ => 0,
        }
    }

    /// The evdev code of a shortcut key name (`KeyCombo::key`).
    pub fn evdev_key(name: &str) -> Option<u16> {
        let named = [("End", 107), ("Home", 102), ("Pause", 119), ("Delete", 111), ("Insert", 110), ("PageUp", 104), ("PageDown", 109)];
        if let Some((_, c)) = named.iter().find(|(n, _)| *n == name) {
            return Some(*c);
        }
        if let Some(f) = name.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()).filter(|f| (13..=24).contains(f)) {
            return Some(183 + f - 13); // KEY_F13
        }
        crate::session::uipad_linux::key_code(name)
    }

    fn mods_of(k: &KeyCombo) -> u8 {
        [(k.ctrl, CTRL), (k.shift, SHIFT), (k.alt, ALT), (k.win, WIN)].iter().filter(|(on, _)| *on).fold(0, |m, (_, b)| m | b)
    }

    struct Wanted {
        mods: u8,
        key: u16,
        tx: UnboundedSender<HubMsg>,
    }

    /// The keys of each shortcut, by `Action::index`.
    static WANTED: Mutex<[Option<Wanted>; 3]> = Mutex::new([None, None, None]);

    pub(super) fn set(action: Action, keys: Option<KeyCombo>, tx: &UnboundedSender<HubMsg>) -> Result<(), String> {
        let wanted = match keys {
            None => None,
            Some(k) => {
                let key = evdev_key(&k.key).ok_or_else(|| format!("{} has no key code on Linux", k.label()))?;
                Some(Wanted { mods: mods_of(&k), key, tx: tx.clone() })
            }
        };
        WANTED.lock().unwrap_or_else(|e| e.into_inner())[action.index()] = wanted;
        Ok(())
    }

    /// A key went down on a keyboard holding the modifiers `mods`. When that is one of the shortcuts (exactly its
    /// modifiers, as on Windows), the hub hears it; true then.
    pub fn key_down(mods: u8, code: u16) -> bool {
        let wanted = WANTED.lock().unwrap_or_else(|e| e.into_inner());
        for action in Action::ALL {
            if let Some(w) = &wanted[action.index()]
                && w.key == code
                && w.mods == mods
            {
                return w.tx.send(action.msg()).is_ok();
            }
        }
        false
    }

    #[cfg(test)]
    pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(test)]
    mod tests {
        use super::*;
        use couchlink_core::shortcut::parse_keys;

        #[test]
        fn the_shortcut_is_its_key_with_exactly_its_modifiers() {
            let _l = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            set(Action::Stop, parse_keys("Ctrl+Shift+Q").unwrap(), &tx).unwrap();
            const Q: u16 = 16;
            assert!(!key_down(CTRL, Q), "Shift is missing");
            assert!(!key_down(CTRL | SHIFT | ALT, Q), "Alt is one too many");
            assert!(!key_down(CTRL | SHIFT, 17), "another key");
            assert!(key_down(CTRL | SHIFT, Q));
            assert!(matches!(rx.try_recv(), Ok(HubMsg::StopShortcut)));
            assert!(rx.try_recv().is_err(), "once");
            set(Action::Stop, parse_keys("Alt+F13").unwrap(), &tx).unwrap();
            assert!(key_down(ALT, 183));
            set(Action::Stop, parse_keys("Ctrl+End").unwrap(), &tx).unwrap();
            assert!(key_down(CTRL, 107));
            set(Action::Stop, None, &tx).unwrap();
            assert!(!key_down(CTRL, 107), "off is off");
            assert_eq!((modifier(97), modifier(54), modifier(100), modifier(126), modifier(16)), (CTRL, SHIFT, ALT, WIN, 0));
        }
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

    const WM_SET: u32 = WM_APP + 1;

    type SetRequest = (Action, Option<KeyCombo>, mpsc::Sender<Result<(), String>>);

    /// The hotkey id of an action (RegisterHotKey ids start at 1).
    fn id_of(action: Action) -> i32 {
        action.index() as i32 + 1
    }

    pub struct Thread {
        id: u32,
        keys: mpsc::Sender<SetRequest>,
    }

    impl Thread {
        pub fn spawn(tx: UnboundedSender<HubMsg>) -> Thread {
            let (keys_tx, keys_rx) = mpsc::channel::<SetRequest>();
            let (id_tx, id_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("hotkey".into())
                .spawn(move || {
                    let mut msg: MSG = unsafe { std::mem::zeroed() };
                    // SAFETY: plain Win32 calls on this thread's own message queue; `msg` is a valid out buffer.
                    unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) }; // makes the queue
                    let _ = id_tx.send(unsafe { GetCurrentThreadId() });
                    let mut registered = [false; 3];
                    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                        match msg.message {
                            WM_HOTKEY => {
                                if let Some(a) = Action::ALL.into_iter().find(|a| msg.wParam == id_of(*a) as usize) {
                                    let _ = tx.send(a.msg());
                                }
                            }
                            WM_SET => {
                                while let Ok((action, keys, reply)) = keys_rx.try_recv() {
                                    let (i, id) = (action.index(), id_of(action));
                                    if registered[i] {
                                        unsafe { UnregisterHotKey(std::ptr::null_mut(), id) };
                                        registered[i] = false;
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
                                            if unsafe { RegisterHotKey(std::ptr::null_mut(), id, mods, k.virtual_key() as u32) } != 0 {
                                                registered[i] = true;
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

        pub fn set(&self, action: Action, keys: Option<KeyCombo>) -> Result<(), String> {
            let (reply_tx, reply_rx) = mpsc::channel();
            self.keys.send((action, keys, reply_tx)).map_err(|_| "the hotkey thread stopped".to_string())?;
            // SAFETY: posting a message to a thread id this module created.
            unsafe { PostThreadMessageW(self.id, WM_SET, 0, 0) };
            reply_rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "the hotkey thread did not answer".to_string())?
        }
    }
}
