//! Couchlink input hook, host side (Windows).
//!
//! Gives one game copy its own controller and keeps it playable next to the other copies. The in-process part is
//! `couchlink_hook.dll` (C++, `hook/`); this crate starts the copy, loads that DLL into it and configures it:
//!
//! ```ignore
//! let copy = couchlink_hook::launch_suspended(exe, "-windowed", [("SteamAppId", "264710")], Some(cwd))?;
//! let hook = couchlink_hook::inject(&copy, &HookConfig { pad: Some(1), focus: true, ..Default::default() })?;
//! couchlink_hook::resume(&copy)?;
//! let status = hook.status()?; // counters: xinput_get_state, cow_broken, ...
//! ```
//!
//! See docs/launch.md ("Input hook") for what each `HookConfig` part does. The DLL is found as described in [`dll`].

pub mod config;
pub mod dll;
pub mod error;
pub mod feed;
pub mod kbmfeed;
pub mod proxy;
#[cfg(target_os = "macos")]
pub mod shm_macos;

pub use config::{HookChild, HookConfig, HookFlags, HookOptions, HookStatus, TestPad};
pub use error::{HookError, Result};

#[cfg(windows)]
mod sec;
#[cfg(windows)]
mod sys;
#[cfg(windows)]
pub use sys::{HookHandle, Launched, inject, inject_with, join_args, launch_suspended, resume};

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub use unsupported::{HookHandle, Launched, inject, inject_with, join_args, launch_suspended, resume};
