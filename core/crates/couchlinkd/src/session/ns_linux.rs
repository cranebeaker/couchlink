//! A seat's sandbox on Linux without any helper program to install. Each seat gets a small holder process
//! (`couchlinkd --seat-namespace`) that makes a user and mount namespace of its own and, inside it:
//!
//! - mounts the seat's copy as a kernel overlay (the install below, the seat's own layer on top; Linux 5.11 and later
//!   allow this without root), so the copy takes no disk and the install is never written,
//! - replaces `/dev/input` with an empty tmpfs that holds only the seat's own nodes, and covers every `/dev/hidraw*`
//!   the seat does not own with `/dev/null` (as `sandbox_linux.rs` does with bwrap),
//! - gives `/tmp/.X11-unix` a folder of its own when the host's belongs to someone else (`own_x11_dir`).
//!
//! The daemon reaches the copy through `/proc/<holder>/root/...` (it owns the namespace, so it may), and the seat's
//! game copies join the holder's namespaces right before they start. The holder ends with the session, or with the
//! daemon (`PR_SET_PDEATHSIG`), and the mounts end with it: nothing is left behind after a crash.
//!
//! Systems that forbid unprivileged user namespaces (Ubuntu 24.04 and later, unless allowed) make `start` fail; the
//! session then falls back to bwrap and fuse-overlayfs or hardlink copies.

use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The argument that makes couchlinkd a holder.
pub const HOLDER_ARG: &str = "--seat-namespace";
/// Starts the holder's answer line (anything else it prints is ignored).
const ANSWER: &str = "couchlink-ns: ";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Overlay {
    pub lower: PathBuf,
    pub upper: PathBuf,
    pub work: PathBuf,
    /// Where the copy appears (the seat's copy folder).
    pub target: PathBuf,
}

/// What a holder sets up.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub overlay: Option<Overlay>,
    /// The seat's own input nodes; every other node in `all` is hidden.
    pub own: Vec<PathBuf>,
    pub all: Vec<PathBuf>,
}

fn cstr(p: &Path) -> Result<CString, String> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| format!("{} has a NUL in it", p.display()))
}

fn errno(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

fn mount(source: Option<&Path>, target: &Path, fstype: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> Result<(), String> {
    let source = source.map(cstr).transpose()?;
    let target_c = cstr(target)?;
    let fstype = fstype.map(|t| CString::new(t).unwrap());
    let data = data.map(|d| CString::new(d).map_err(|_| "bad mount options".to_string())).transpose()?;
    // SAFETY: valid NUL-terminated strings or null pointers, as mount(2) takes them.
    let r = unsafe {
        libc::mount(
            source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            target_c.as_ptr(),
            fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            flags,
            data.as_ref().map_or(std::ptr::null(), |s| s.as_ptr() as *const libc::c_void),
        )
    };
    if r == 0 { Ok(()) } else { Err(errno(&format!("mount {}", target.display()))) }
}

/// One overlay option value: commas and backslashes are escaped.
fn opt(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\").replace(',', "\\,")
}

/// Enter a user and mount namespace of our own, mapped to our own user and group.
fn own_namespaces() -> Result<(), String> {
    // SAFETY: plain process calls.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    // SAFETY: unshare has no memory arguments.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) } != 0 {
        return Err(errno("this system does not allow user namespaces"));
    }
    std::fs::write("/proc/self/setgroups", "deny").map_err(|e| format!("setgroups: {e}"))?;
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1")).map_err(|e| format!("uid_map: {e}"))?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1")).map_err(|e| format!("gid_map: {e}"))?;
    // Nothing done in here may reach the rest of the system.
    mount(None, Path::new("/"), None, libc::MS_REC | libc::MS_PRIVATE, None)
}

fn mount_overlay(o: &Overlay) -> Result<(), String> {
    let base = format!("lowerdir={},upperdir={},workdir={}", opt(&o.lower), opt(&o.upper), opt(&o.work));
    // userxattr keeps overlay's bookkeeping in user.* attributes, which an unprivileged mount may write.
    mount(Some(Path::new("overlay")), &o.target, Some("overlay"), 0, Some(&format!("{base},userxattr")))
        .or_else(|_| mount(Some(Path::new("overlay")), &o.target, Some("overlay"), 0, Some(&base)))
}

/// A fresh /dev/input with only `own` in it, and every other hidraw node covered.
fn mask_input(own: &[PathBuf], all: &[PathBuf]) -> Result<(), String> {
    let input = Path::new("/dev/input");
    if input.is_dir() {
        // Hold the nodes we keep before the tmpfs hides them; they are bound back from these handles.
        let mut keep = Vec::new();
        for n in own.iter().filter(|n| n.starts_with(input)) {
            let c = cstr(n)?;
            // SAFETY: a valid path; the descriptor is owned below.
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if fd >= 0 {
                // SAFETY: fd is a new descriptor nobody else owns.
                keep.push((n.clone(), unsafe { OwnedFd::from_raw_fd(fd) }));
            }
        }
        mount(Some(Path::new("tmpfs")), input, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NOEXEC, Some("mode=0755"))?;
        for (node, fd) in keep {
            File::create(&node).map_err(|e| format!("{}: {e}", node.display()))?;
            mount(Some(&PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))), &node, None, libc::MS_BIND, None)?;
        }
    }
    for n in all.iter().filter(|n| !n.starts_with(input) && !own.contains(n)) {
        mount(Some(Path::new("/dev/null")), n, None, libc::MS_BIND, None)?;
    }
    Ok(())
}

/// The folder X servers keep their sockets in.
pub const X11_DIR: &str = "/tmp/.X11-unix";

/// The X sockets in `dir` (X0, X1 and so on), sorted.
pub fn x11_sockets(dir: &Path) -> Vec<PathBuf> {
    use std::os::unix::fs::FileTypeExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        // stat, not the entry's own type: a socket bound over a file (in a seat's folder) shows as that file there.
        .filter(|e| e.file_name().to_string_lossy().starts_with('X') && std::fs::metadata(e.path()).is_ok_and(|m| m.file_type().is_socket()))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// True when `/tmp/.X11-unix` exists and belongs to someone other than this user. On most desktops it is root's; in a
/// seat's namespace only this user is mapped, so it shows as nobody's there, and wlroots (gamescope's Xwayland) will
/// not put its X socket in a folder that is neither root's nor its own.
pub fn x11_dir_foreign() -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: a plain process call.
    std::fs::metadata(X11_DIR).is_ok_and(|m| m.uid() != unsafe { libc::getuid() })
}

/// Inside the namespace: a fresh `/tmp/.X11-unix` of this user's, with the host's X sockets bound back in, so gamescope
/// still reaches the desktop's display and its Xwayland may add a socket of its own. The host's folder is untouched.
fn own_x11_dir() -> Result<(), String> {
    if !x11_dir_foreign() {
        return Ok(());
    }
    let dir = Path::new(X11_DIR);
    // Hold the sockets before the tmpfs hides them; they are bound back from these handles.
    let mut keep = Vec::new();
    for s in x11_sockets(dir) {
        let c = cstr(&s)?;
        // SAFETY: a valid path; the descriptor is owned below.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd >= 0 {
            // SAFETY: fd is a new descriptor nobody else owns.
            keep.push((s, unsafe { OwnedFd::from_raw_fd(fd) }));
        }
    }
    mount(Some(Path::new("tmpfs")), dir, Some("tmpfs"), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, Some("mode=1777"))?;
    for (socket, fd) in keep {
        File::create(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
        mount(Some(&PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))), &socket, None, libc::MS_BIND, None)?;
    }
    Ok(())
}

/// End this process when its parent ends. Set after any change of user namespace: changing credentials clears it.
/// If the parent is already gone (it ended before this ran), end now.
fn die_with(parent: libc::pid_t) {
    // SAFETY: plain process calls.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
        if libc::getppid() != parent {
            libc::_exit(1);
        }
    }
}

/// Everything a holder does before it reports back; true when the overlay is mounted.
fn setup(cfg: &Config) -> Result<(bool, String), String> {
    // SAFETY: a plain process call.
    let parent = unsafe { libc::getppid() };
    own_namespaces()?;
    // The holder ends with whoever started it (the daemon's session, or the daemon itself when it dies).
    die_with(parent);
    let (overlay, why) = match &cfg.overlay {
        Some(o) => match mount_overlay(o) {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        },
        None => (false, String::new()),
    };
    mask_input(&cfg.own, &cfg.all)?;
    own_x11_dir().map_err(|e| format!("{X11_DIR}: {e}"))?;
    Ok((overlay, why))
}

/// The holder process: reads its config (one JSON line on stdin), sets up, answers `ok <overlay> <pid>` or `error <why>`,
/// then waits until stdin closes (the session ends, or the daemon dies).
pub fn holder_main() -> i32 {
    let mut stdin = std::io::stdin().lock();
    let mut line = String::new();
    if stdin.read_line(&mut line).is_err() {
        return 2;
    }
    let mut out = std::io::stdout().lock();
    let answer = match serde_json::from_str::<Config>(&line).map_err(|e| format!("bad config: {e}")).and_then(|c| setup(&c)) {
        // The process whose namespaces these are (the daemon reaches the copy through its /proc entry).
        Ok((overlay, why)) => format!("ok {overlay} {} {why}", std::process::id()),
        Err(e) => format!("error {e}"),
    };
    let failed = answer.starts_with("error");
    let _ = writeln!(out, "{ANSWER}{}", answer.replace('\n', " "));
    let _ = out.flush();
    drop(out);
    if failed {
        return 1;
    }
    let mut buf = [0u8; 64];
    while let Ok(n) = stdin.read(&mut buf) {
        if n == 0 {
            break;
        }
    }
    0
}

/// A running holder. Dropping it ends the namespace and everything mounted in it.
pub struct SeatNs {
    child: Child,
    pub pid: u32,
    /// The kernel overlay is mounted at the config's target.
    pub overlay: bool,
    /// Why the overlay could not be mounted, when it was asked for.
    pub overlay_error: String,
}

impl SeatNs {
    /// Start a holder (the program `exe`, normally couchlinkd itself) with `cfg`.
    pub fn start(exe: &Path, cfg: &Config) -> Result<SeatNs, String> {
        Self::start_with(Command::new(exe).arg(HOLDER_ARG), cfg)
    }

    pub fn start_with(cmd: &mut Command, cfg: &Config) -> Result<SeatNs, String> {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("cannot start the seat sandbox: {e}"))?;
        let pid = child.id();
        let line = serde_json::to_string(cfg).expect("config serialises");
        let mut stdin = child.stdin.take().expect("piped");
        let _ = writeln!(stdin, "{line}");
        child.stdin = Some(stdin);
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout);
            let mut answer = String::new();
            loop {
                answer.clear();
                match lines.read_line(&mut answer) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if answer.starts_with(ANSWER) => {
                        let _ = tx.send(answer[ANSWER.len()..].to_string());
                        return;
                    }
                    Ok(_) => {}
                }
            }
            let _ = tx.send(String::new());
        });
        let answer = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
        let mut ns = SeatNs { child, pid, overlay: false, overlay_error: String::new() };
        let answer = answer.trim();
        if let Some(rest) = answer.strip_prefix("ok ") {
            let mut parts = rest.splitn(3, ' ');
            ns.overlay = parts.next() == Some("true");
            if let Some(p) = parts.next().and_then(|p| p.parse().ok()) {
                ns.pid = p;
            }
            ns.overlay_error = parts.next().unwrap_or("").trim().to_string();
            return Ok(ns);
        }
        let why = answer.strip_prefix("error ").unwrap_or(if answer.is_empty() { "the seat sandbox did not answer" } else { answer }).to_string();
        Err(why)
    }

    /// The holder's view of the file system, as the daemon reaches it.
    pub fn root(&self) -> PathBuf {
        root_of(self.pid)
    }

    /// `path` as the namespace sees it, reached from outside.
    pub fn view(&self, path: &Path) -> PathBuf {
        let rel = path.strip_prefix("/").unwrap_or(path);
        self.root().join(rel)
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for SeatNs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `/proc/<pid>/root`.
pub fn root_of(pid: u32) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/root"))
}

/// `s` with the namespace prefix `/proc/<pid>/root` taken out, as the copy itself sees it.
pub fn inside(s: &str, pid: u32) -> String {
    let prefix = format!("/proc/{pid}/root");
    let out = s.replace(&format!("{prefix}/"), "/");
    if out == prefix { "/".to_string() } else { out }
}

/// For `Command::pre_exec`: join the holder `pid`'s user and mount namespaces, go to `cwd` there, and die with the
/// daemon. The files are opened now, in the daemon.
pub fn joiner(pid: u32, cwd: &Path) -> Result<impl FnMut() -> std::io::Result<()> + Send + Sync + 'static, String> {
    let user = File::open(format!("/proc/{pid}/ns/user")).map_err(|e| format!("the seat sandbox is gone: {e}"))?;
    let mnt = File::open(format!("/proc/{pid}/ns/mnt")).map_err(|e| format!("the seat sandbox is gone: {e}"))?;
    let cwd = cstr(cwd)?;
    Ok(move || {
        // SAFETY: only system calls on descriptors and a string prepared before the fork.
        unsafe {
            let parent = libc::getppid();
            if libc::setns(user.as_raw_fd(), libc::CLONE_NEWUSER) != 0 || libc::setns(mnt.as_raw_fd(), libc::CLONE_NEWNS) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // After the namespace change, which clears it: the copy ends with couchlinkd.
            die_with(parent);
            if libc::chdir(cwd.as_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    })
}

/// True when a program inside the holder `pid` may make a user namespace of its own (Proton's runtime does).
pub fn nested_allowed(pid: u32) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut join = joiner(pid, Path::new("/"))?;
    let mut cmd = Command::new("true");
    // SAFETY: the closure only makes system calls.
    unsafe {
        cmd.pre_exec(move || {
            join()?;
            if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("a sandbox inside the seat's sandbox is not allowed".to_string()),
        Err(e) => Err(format!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_the_namespace() {
        assert_eq!(inside("/proc/42/root/home/p/game/x.exe", 42), "/home/p/game/x.exe");
        assert_eq!(inside("--out=/proc/42/root/s/status.json", 42), "--out=/s/status.json");
        assert_eq!(inside("/proc/42/root", 42), "/");
        assert_eq!(inside("/proc/421/root/x", 42), "/proc/421/root/x", "another holder's paths are left alone");
        assert_eq!(opt(Path::new("/g/a,b")), r"/g/a\,b");
    }

    /// Runs as a holder when the test below starts this test binary with COUCHLINK_TEST_HOLDER=1.
    #[test]
    #[ignore]
    fn test_holder() {
        if std::env::var("COUCHLINK_TEST_HOLDER").is_ok_and(|v| v == "1") {
            // libtest runs this on a thread of its own, and only a process with one thread may make a user namespace,
            // so the holder is a fork (couchlinkd's real holder is single-threaded already).
            // SAFETY: the child only runs the holder and exits; the parent waits for it.
            let pid = unsafe { libc::fork() };
            if pid == 0 {
                std::process::exit(holder_main());
            }
            let mut status = 0;
            // SAFETY: waiting for our own child.
            unsafe { libc::waitpid(pid, &mut status, 0) };
            std::process::exit(if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 1 });
        }
    }

    /// Starts a holder through this test binary.
    pub(crate) fn start_test_holder(cfg: &Config) -> Result<SeatNs, String> {
        let exe = std::env::current_exe().unwrap();
        SeatNs::start_with(
            Command::new(exe).args(["--exact", "session::ns_linux::tests::test_holder", "--ignored", "--test-threads=1", "--nocapture", "-q"]).env("COUCHLINK_TEST_HOLDER", "1"),
            cfg,
        )
    }

    /// gamescope starts its Xwayland in a seat's namespace when the host's /tmp/.X11-unix is root's (SteamOS and most
    /// desktops). Skipped without gamescope or user namespaces, unless COUCHLINK_REQUIRE_X11_DIR_TEST=1: CI runs it as
    /// a normal user with a root-owned folder.
    #[test]
    fn gamescope_starts_its_xwayland_in_a_seat_namespace() {
        use std::os::unix::process::CommandExt;
        let required = std::env::var("COUCHLINK_REQUIRE_X11_DIR_TEST").is_ok_and(|v| v == "1");
        let skip = |why: String| {
            assert!(!required, "{why}");
            eprintln!("skipped gamescope_starts_its_xwayland_in_a_seat_namespace: {why}");
        };
        let Some(gamescope) = crate::session::sandbox_linux::which("gamescope") else { return skip("no gamescope".into()) };
        if required {
            assert!(x11_dir_foreign(), "{X11_DIR} must belong to another user for this test");
        }
        let ns = match start_test_holder(&Config::default()) {
            Ok(ns) => ns,
            Err(e) => return skip(e),
        };
        let dir = std::env::temp_dir().join(format!("couchlink-x11-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shown = dir.join("display");
        let mut cmd = Command::new(gamescope);
        cmd.args(["--backend", "headless", "-W", "320", "-H", "240", "--", "sh", "-c", "printf %s \"$DISPLAY\" > \"$0\"; sleep 2"])
            .arg(&shown)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(dir.join("gamescope.log")).unwrap());
        let mut join = joiner(ns.pid, &dir).unwrap();
        // SAFETY: the closure only makes system calls.
        unsafe { cmd.pre_exec(move || join()) };
        let mut child = cmd.spawn().unwrap();
        let t = std::time::Instant::now();
        while std::fs::read_to_string(&shown).map_or(true, |s| s.is_empty()) && t.elapsed() < Duration::from_secs(30) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        let display = std::fs::read_to_string(&shown).unwrap_or_default();
        let log = std::fs::read_to_string(dir.join("gamescope.log")).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect();
        assert!(display.starts_with(':'), "gamescope started its Xwayland in the namespace; its log ends:\n{}", tail.join("\n"));
        if x11_dir_foreign() {
            use std::os::unix::fs::MetadataExt;
            let inside = std::fs::metadata(ns.view(Path::new(X11_DIR))).unwrap();
            // SAFETY: a plain process call.
            assert_eq!(inside.uid(), unsafe { libc::getuid() }, "the namespace has a folder of its own");
            let host = x11_sockets(Path::new(X11_DIR));
            let seen = x11_sockets(&ns.view(Path::new(X11_DIR)));
            assert!(host.iter().all(|h| seen.iter().any(|s| s.file_name() == h.file_name())), "with the host's sockets in it: {host:?} {seen:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A holder with an overlay: the copy shows the install, takes changes without writing the install, keeps them in
    /// the upper layer, and is gone with the holder. Skipped where user namespaces are not allowed, unless
    /// COUCHLINK_REQUIRE_SANDBOX_TEST=1.
    #[test]
    fn a_holder_mounts_the_copy_as_a_kernel_overlay() {
        let required = std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1");
        let root = std::env::temp_dir().join(format!("couchlink-ns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (game, upper, work, copy) = (root.join("game"), root.join("upper"), root.join("work"), root.join("copy"));
        for d in [&game, &upper, &work, &copy] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(game.join("a.txt"), "install").unwrap();
        let cfg = Config { overlay: Some(Overlay { lower: game.clone(), upper: upper.clone(), work, target: copy.clone() }), own: vec![], all: vec![] };
        let ns = match start_test_holder(&cfg) {
            Ok(ns) => ns,
            Err(e) => {
                assert!(!required, "{e}");
                eprintln!("skipped a_holder_mounts_the_copy_as_a_kernel_overlay: {e}");
                return;
            }
        };
        assert!(ns.overlay, "the overlay is mounted ({})", ns.overlay_error);
        let view = ns.view(&copy);
        assert_eq!(std::fs::read_to_string(view.join("a.txt")).unwrap(), "install");
        std::fs::write(view.join("a.txt"), "mine").unwrap();
        std::fs::write(view.join("save.dat"), "save").unwrap();
        assert_eq!(std::fs::read_to_string(game.join("a.txt")).unwrap(), "install", "the install is never written");
        assert_eq!(std::fs::read_to_string(upper.join("a.txt")).unwrap(), "mine");
        assert!(std::fs::read_dir(&copy).unwrap().next().is_none(), "outside the namespace the copy folder is untouched");
        assert_eq!(nested_allowed(ns.pid), Ok(()), "a sandbox inside the seat's sandbox works here");
        let pid = ns.pid;
        drop(ns);
        // The namespace ends with its holder (which may take a moment to go).
        let gone = || !Path::new(&format!("/proc/{pid}")).exists() || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "));
        let t = std::time::Instant::now();
        while !gone() && t.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let state: Vec<&str> = status.lines().filter(|l| l.starts_with("State:") || l.starts_with("PPid:")).collect();
        assert!(gone(), "the holder ended ({state:?})");
        assert_eq!(std::fs::read_to_string(upper.join("save.dat")).unwrap(), "save", "the seat's changes stay in its layer");
        let _ = std::fs::remove_dir_all(&root);
    }
}
