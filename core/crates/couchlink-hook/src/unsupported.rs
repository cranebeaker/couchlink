//! The hook is Windows only. These stand-ins let the workspace build elsewhere; every call returns `Unsupported`.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use crate::config::{HookConfig, HookOptions, HookStatus};
use crate::error::{HookError, Result};

fn no() -> HookError {
    HookError::Unsupported("the input hook is only available on Windows".into())
}

#[derive(Debug)]
pub struct Launched;

impl Launched {
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn is_x86(&self) -> bool {
        false
    }
    pub fn exit_code(&self) -> Option<u32> {
        None
    }
    pub fn is_running(&self) -> bool {
        false
    }
    pub fn wait(&self, _timeout: Duration) -> Option<u32> {
        None
    }
    pub fn terminate(&self) {}
}

#[derive(Debug)]
pub struct HookHandle;

impl HookHandle {
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn connect(_pid: u32, _timeout: Duration) -> Result<HookHandle> {
        Err(no())
    }
    pub fn initial_status(&self) -> &HookStatus {
        unreachable!("a HookHandle cannot be created on this platform")
    }
    pub fn status(&self) -> Result<HookStatus> {
        Err(no())
    }
    pub fn ping(&self) -> bool {
        false
    }
}

pub fn join_args<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
}

pub fn launch_suspended<I, K, V>(_exe: &Path, _args: &str, _env: I, _cwd: Option<&Path>) -> Result<Launched>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    Err(no())
}

pub fn resume(_l: &Launched) -> Result<()> {
    Err(no())
}

pub fn inject(_l: &Launched, _config: &HookConfig) -> Result<HookHandle> {
    Err(no())
}

pub fn inject_with(_l: &Launched, _config: &HookConfig, _options: &HookOptions) -> Result<HookHandle> {
    Err(no())
}
