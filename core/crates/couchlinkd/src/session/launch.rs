//! Starting game processes. `Launcher` is the seam the input hook plugs into: the default launcher just
//! starts the process; the hook launcher starts it suspended, injects, and resumes (docs/launch.md).

use couchlink_core::recipe::{Input, Wait};
use couchlink_core::Journal;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Everything needed to start one process.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// The seat this copy is for; none for the `first` helper.
    pub seat: Option<usize>,
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// Added to the environment Couchlink itself runs in.
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// This copy's folder (the real game folder for the helper).
    pub game_dir: PathBuf,
    /// Device ids seated at this seat (for giving the copy only its own controller).
    pub devices: Vec<String>,
    /// Single-instance lock names to rename (recipe `locks`).
    pub locks: Vec<String>,
    pub input: Input,
    /// Save redirects for the hook: (the path the game uses, this seat's own folder for it).
    pub saves: Vec<(PathBuf, PathBuf)>,
    /// The exe is a launcher that starts the real game: the hook follows every process the copy starts.
    pub hook_children: bool,
    /// The seat plays on its own Steam account: the copy runs as that seat's helper Windows user.
    pub run_as: Option<couchlink_steam::AccountsConfig>,
}

/// A started process. Dropping it must not leave processes behind unless they were stopped by `kill`.
pub trait Child: Send {
    fn pid(&self) -> u32;
    /// `Ok(Some(code))` once it has exited; `Ok(None)` while it runs.
    fn try_wait(&mut self) -> Result<Option<i32>, String>;
    /// End the process and everything it started. Does not wait.
    fn kill(&mut self);
}

pub struct Launched {
    pub child: Box<dyn Child>,
    /// True when the input hook is in the process (the session then reports the `hook` step).
    pub hooked: bool,
}

/// Starts processes. Must not block for long: the session worker waits on it.
pub trait Launcher: Send + Sync {
    /// Called right before `launch` for a game copy. A launcher that changes something outside its own process (a
    /// file in the game folder) records that change in `journal` here, BEFORE it happens, so a crash cannot leave it.
    fn prepare(&self, _spec: &LaunchSpec, _journal: &mut Journal) -> Result<(), String> {
        Ok(())
    }

    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String>;
}

/// The default launcher: `std::process::Command` with the spec's arguments, environment and folder. On
/// Windows every process runs in a job object, so killing it kills its children too, and a crashed
/// daemon does not leave games running.
pub struct CommandLauncher;

impl Launcher for CommandLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String> {
        let mut cmd = Command::new(&spec.exe);
        cmd.args(&spec.args).envs(&spec.env).current_dir(&spec.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", spec.exe.display()))?;
        let job = job::Job::adopt(&child);
        Ok(Launched { child: Box::new(Started { child, job }), hooked: false })
    }
}

struct Started {
    child: std::process::Child,
    job: Option<job::Job>,
}

impl Child for Started {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.code().unwrap_or(-1))),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("cannot check on process {}: {e}", self.child.id())),
        }
    }

    fn kill(&mut self) {
        if let Some(j) = &self.job {
            j.terminate();
        }
        let _ = self.child.kill();
    }
}

#[cfg(windows)]
pub(super) mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;

    /// A job object that kills everything in it when closed.
    pub struct Job(HANDLE);

    // SAFETY: a job handle can be used from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub fn adopt(child: &std::process::Child) -> Option<Job> {
            Self::adopt_handle(child.as_raw_handle() as HANDLE)
        }

        /// Same as `adopt` for a process known only by its id (one the input hook started).
        pub fn adopt_pid(pid: u32) -> Option<Job> {
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
            // SAFETY: plain Win32 calls; the process handle is closed here, the job keeps the process.
            unsafe {
                let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if p.is_null() {
                    return None;
                }
                let job = Self::adopt_handle(p);
                CloseHandle(p);
                job
            }
        }

        pub fn adopt_handle(process: HANDLE) -> Option<Job> {
            // SAFETY: plain Win32 calls; the handle is closed in Drop.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 || AssignProcessToJobObject(h, process) == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h))
            }
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is valid until Drop.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing the handle we own; the job's processes die with it.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
pub(super) mod job {
    pub struct Job;

    impl Job {
        pub fn adopt_pid(_: u32) -> Option<Job> {
            None
        }

        pub fn adopt(_: &std::process::Child) -> Option<Job> {
            None
        }
        pub fn terminate(&self) {}
    }
}

/// How long a `port:N` wait lasts before giving up.
pub const PORT_WAIT: Duration = Duration::from_secs(60);

/// True when something listens on TCP or UDP port `port` on this PC.
pub fn port_in_use(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok() {
        return true;
    }
    // A UDP port cannot be connected to; if we cannot bind it, someone else has it. Windows lets us bind
    // 127.0.0.1 next to a server on 0.0.0.0 or [::] (Nitrox listens on both), so try the wildcards too.
    let taken = |a: SocketAddr| {
        matches!(UdpSocket::bind(a), Err(e) if matches!(e.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied))
    };
    taken(addr) || taken(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) || taken(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)))
}

/// Wait for the helper's condition. Gives up early if the helper exits (except for `exit`, which waits for
/// that), `stop` is set, or `timeout` passes.
pub fn wait_for(wait: &Wait, helper: &mut dyn Child, stop: &AtomicBool, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".to_string());
        }
        if let Some(code) = helper.try_wait()? {
            // A delay only needs the helper to be alive for its duration; the others need it to survive until they hold.
            if matches!(wait, Wait::Exit) {
                return Ok(());
            }
            return Err(format!("the helper exited (code {code}) before {}", describe(wait)));
        }
        let held = match wait {
            Wait::Delay(ms) => start.elapsed() >= Duration::from_millis(*ms),
            Wait::Port(p) => port_in_use(*p),
            Wait::File(f) => std::path::Path::new(f).exists(),
            Wait::Window(t) => super::window::window_titled(t),
            Wait::Exit => false,
        };
        if held {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(format!("timed out after {} s waiting for {}", timeout.as_secs(), describe(wait)));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn describe(w: &Wait) -> String {
    match w {
        Wait::Port(p) => format!("port {p} opened"),
        Wait::Delay(ms) => format!("the {ms} ms delay ended"),
        Wait::File(f) => format!("{f} appeared"),
        Wait::Window(t) => format!("a window titled \"{t}\" appeared"),
        Wait::Exit => "it finished".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A child that has already exited, or never does.
    struct Fake(Option<i32>);

    impl Child for Fake {
        fn pid(&self) -> u32 {
            1
        }
        fn try_wait(&mut self) -> Result<Option<i32>, String> {
            Ok(self.0)
        }
        fn kill(&mut self) {}
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn a_listening_tcp_port_is_seen() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_in_use(port));
        let stop = AtomicBool::new(false);
        assert_eq!(wait_for(&Wait::Port(port), &mut Fake(None), &stop, Duration::from_secs(5)), Ok(()));
        drop(l);
        assert!(!port_in_use(port));
    }

    #[test]
    fn a_bound_udp_port_is_seen() {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = s.local_addr().unwrap().port();
        assert!(port_in_use(port));
        drop(s);
        assert!(!port_in_use(port));
    }

    #[test]
    fn waiting_ends_on_exit_stop_or_timeout() {
        let stop = AtomicBool::new(false);
        let port = free_port();
        let e = wait_for(&Wait::Port(port), &mut Fake(Some(3)), &stop, Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("exited (code 3)") && e.contains(&port.to_string()), "{e}");
        let e = wait_for(&Wait::Port(port), &mut Fake(None), &stop, Duration::from_millis(200)).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        stop.store(true, Ordering::Relaxed);
        assert_eq!(wait_for(&Wait::Delay(10_000), &mut Fake(None), &stop, Duration::from_secs(20)), Err("stopped".into()));
    }

    #[test]
    fn files_and_exits_can_be_waited_for() {
        let stop = AtomicBool::new(false);
        let dir = std::env::temp_dir().join(format!("couchlinkd-wait-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let flag = dir.join("ready.txt");
        let _ = std::fs::remove_file(&flag);
        let wait = Wait::File(flag.display().to_string());
        let e = wait_for(&wait, &mut Fake(None), &stop, Duration::from_millis(150)).unwrap_err();
        assert!(e.contains("timed out") && e.contains("ready.txt"), "{e}");
        std::fs::write(&flag, "x").unwrap();
        assert_eq!(wait_for(&wait, &mut Fake(None), &stop, Duration::from_secs(5)), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
        // `exit` is the one wait that wants the helper gone.
        assert_eq!(wait_for(&Wait::Exit, &mut Fake(Some(0)), &stop, Duration::from_secs(5)), Ok(()));
        assert!(wait_for(&Wait::Exit, &mut Fake(None), &stop, Duration::from_millis(150)).unwrap_err().contains("timed out"));
        // A window nobody has opened is never found.
        let title = format!("couchlink-no-such-window-{}", std::process::id());
        assert!(wait_for(&Wait::Window(title), &mut Fake(None), &stop, Duration::from_millis(150)).unwrap_err().contains("timed out"));
    }

    #[test]
    fn a_delay_waits_that_long() {
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        assert_eq!(wait_for(&Wait::Delay(300), &mut Fake(None), &stop, Duration::from_secs(5)), Ok(()));
        assert!(t.elapsed() >= Duration::from_millis(300) && t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn the_command_launcher_starts_and_kills_a_process() {
        // `ping -n 30` (Windows) / `sleep 30` run long enough to be killed.
        let (exe, args): (&str, Vec<String>) = if cfg!(windows) { ("ping", vec!["-n".into(), "30".into(), "127.0.0.1".into()]) } else { ("sleep", vec!["30".into()]) };
        let spec = LaunchSpec {
            seat: Some(0),
            exe: exe.into(),
            args,
            env: BTreeMap::from([("X".to_string(), "1".to_string())]),
            cwd: std::env::temp_dir(),
            game_dir: std::env::temp_dir(),
            devices: vec![],
            locks: vec![],
            input: Input::default(),
            saves: vec![],
            hook_children: false,
            run_as: None,
        };
        let mut l = CommandLauncher.launch(&spec).unwrap();
        assert!(!l.hooked);
        assert_eq!(l.child.try_wait().unwrap(), None);
        l.child.kill();
        let t = Instant::now();
        let code = loop {
            if let Some(c) = l.child.try_wait().unwrap() {
                break c;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "process did not die");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_ne!(code, 0);
        let missing = LaunchSpec { exe: "definitely-not-a-program.exe".into(), ..spec };
        assert!(CommandLauncher.launch(&missing).err().unwrap().contains("cannot start"));
    }
}
