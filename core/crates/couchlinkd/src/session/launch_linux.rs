//! Starting game copies on Linux (docs/launch.md, "Session pipeline (Linux)"). A copy is, from the outside in:
//!
//! 1. the seat's sandbox: it joins the namespace of its seat's holder (`ns_linux.rs`: its layered copy, and only its
//!    own input devices), or, where the system does not allow that, runs in bwrap (`sandbox_linux.rs`),
//! 2. `gamescope`, when installed: a window of the seat's size of its own; the copy keeps getting its controller's input
//!    while another copy's window has the focus, because it reads the controller itself, and the seat's own keyboard
//!    and mouse reach it through XTEST (`rawkbm_linux.rs`). Without gamescope the copy's own window is placed,
//! 3. Proton for Windows games, with a prefix of the seat's own: through umu-launcher when it is installed, else
//!    Steam's own Proton; or the native program, with a home of the seat's own.
//!
//! Everything a copy starts is in its own process group, which is what `kill` ends, and dies with couchlinkd.

use super::launch::{Child, LaunchSpec, Launched, Launcher, LinuxSeat};
use super::sandbox_linux::{seat_args, which};
use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct LinuxLauncher;

impl Launcher for LinuxLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String> {
        let tools = Tools::find();
        // A copy's own output (gamescope, Proton, the game) goes to `copy.log` next to its home, for when it fails.
        let mut log = None;
        let mut kbm = None;
        let mut join = None;
        let line = match &spec.linux {
            Some(seat) => {
                let home = seat.data_dir.join("home");
                std::fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
                // Native Steam games find the Steam client through ~/.steam, so the seat's home points at the real one.
                if let Some(real) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".steam")).filter(|p| p.exists()) {
                    if std::fs::symlink_metadata(home.join(".steam")).is_err() {
                        let _ = std::os::unix::fs::symlink(real, home.join(".steam"));
                    }
                }
                let path = spec.log.clone().unwrap_or_else(|| seat.data_dir.join("copy.log"));
                let _ = path.parent().map(std::fs::create_dir_all);
                log = std::fs::File::create(path).ok();
                let sandbox = match seat.sandbox_pid {
                    Some(pid) => Sandbox::Holder(pid),
                    None => Sandbox::Bwrap,
                };
                let line = copy_line(spec, seat, &tools, &super::hook::pad_nodes(&spec.devices), sandbox)?;
                if line.umu {
                    // Proton's runtime starts a sandbox of its own inside the seat's.
                    match sandbox {
                        Sandbox::Holder(pid) => super::ns_linux::nested_allowed(pid).map_err(|e| apparmor(&e))?,
                        Sandbox::Bwrap => nested_sandbox(tools.bwrap.as_deref().unwrap_or(Path::new("bwrap")))?,
                    }
                }
                if let Some(file) = &line.display_file {
                    let _ = std::fs::remove_file(file);
                    kbm = super::rawkbm::linux::attach_x(&spec.devices, file.clone());
                    // What the seat's profile turns into keys and mouse input goes to the same display.
                    if let Some(seat) = spec.seat {
                        super::uipad_linux::set_display(seat, file.clone());
                    }
                } else if spec.devices.iter().any(|d| super::rawkbm::is_kbm_id(d)) {
                    eprintln!("couchlinkd: seat {:?} has a keyboard or mouse of its own, but runs without gamescope; it gets the desktop's", spec.seat);
                }
                if let Sandbox::Holder(pid) = sandbox {
                    join = Some(super::ns_linux::joiner(pid, Path::new(&super::ns_linux::inside(&spec.cwd.display().to_string(), pid)))?);
                }
                line
            }
            None => helper_line(spec, &tools)?,
        };
        let mut cmd = Command::new(&line.program);
        cmd.args(&line.args).envs(&line.env).current_dir(&spec.cwd).stdin(Stdio::null());
        match log.as_ref().and_then(|f| Some((f.try_clone().ok()?, f.try_clone().ok()?))) {
            Some((out, err)) => cmd.stdout(out).stderr(err),
            None => cmd.stdout(Stdio::null()).stderr(Stdio::null()),
        };
        cmd.process_group(0);
        if let Some(join) = join {
            // SAFETY: the closure only makes system calls (ns_linux::joiner).
            unsafe { cmd.pre_exec(join) };
        }
        let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", line.program.display()))?;
        Ok(Launched { child: Box::new(Group { child, _kbm: kbm }), hooked: false })
    }
}

/// How a copy is kept to its own devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    /// It joins its seat's holder process's namespaces (`ns_linux.rs`).
    Holder(u32),
    /// It runs in bwrap.
    Bwrap,
}

/// The programs a copy may run through, as found on `PATH`.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub bwrap: Option<PathBuf>,
    pub gamescope: Option<PathBuf>,
    pub umu: Option<PathBuf>,
}

impl Tools {
    pub fn find() -> Tools {
        Tools { bwrap: which("bwrap"), gamescope: which("gamescope"), umu: which("umu-run") }
    }
}

/// One command line: the program, its arguments, and the environment added to couchlinkd's own.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// It runs through umu-launcher (whose runtime is a sandbox of its own).
    pub umu: bool,
    /// Where the copy writes gamescope's display name, when it runs in gamescope.
    pub display_file: Option<PathBuf>,
}

/// What a helper (`launch.first`, per-seat helpers) runs as: Windows programs through Proton in the game's shared
/// prefix, others as they are. Helpers are not sandboxed and get no gamescope window.
fn helper_line(spec: &LaunchSpec, tools: &Tools) -> Result<Line, String> {
    let exe = spec.exe.display().to_string();
    let mut env = spec.env.clone();
    if !couchlink_core::recipe_linux::LinuxOpts::default().uses_proton(&exe) {
        return Ok(Line { program: spec.exe.clone(), args: spec.args.clone(), env, umu: false, display_file: None });
    }
    let data = spec.game_dir.join(".couchlink-prefix");
    let (program, mut args, umu) = proton_command(tools, "", "umu-0", None, &data, &mut env)?;
    args.push(exe);
    args.extend(spec.args.iter().cloned());
    Ok(Line { program, args, env, umu, display_file: None })
}

/// How a Windows program starts: umu-run when installed (prefix `<data>/prefix`), else Steam's own Proton with
/// `proton waitforexitandrun` (prefix `<data>/compat/pfx`). Gives the program and the arguments before the exe.
fn proton_command(tools: &Tools, choice: &str, game_id: &str, app_id: Option<u32>, data: &Path, env: &mut BTreeMap<String, String>) -> Result<(PathBuf, Vec<String>, bool), String> {
    // Wine's controller driver reads hidraw too; the sandbox has hidden every other seat's hidraw nodes.
    env.entry("PROTON_ENABLE_HIDRAW".into()).or_insert_with(|| "1".into());
    if let Some(umu) = &tools.umu {
        env.insert("WINEPREFIX".into(), data.join("prefix").display().to_string());
        env.insert("GAMEID".into(), game_id.to_string());
        env.entry("STORE".into()).or_insert_with(|| "none".into());
        match proton_path(choice)? {
            Some(p) => env.insert("PROTONPATH".into(), p),
            None => env.remove("PROTONPATH"),
        };
        return Ok((umu.clone(), Vec::new(), true));
    }
    let proton = steam_proton(choice).ok_or(NO_PROTON)?;
    let steam = couchlink_steam::steam_root().ok_or(NO_PROTON)?;
    let compat = data.join("compat");
    let _ = std::fs::create_dir_all(&compat);
    env.insert("STEAM_COMPAT_DATA_PATH".into(), compat.display().to_string());
    env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), steam.display().to_string());
    let id = app_id.unwrap_or(0).to_string();
    env.entry("SteamAppId".into()).or_insert_with(|| id.clone());
    env.entry("SteamGameId".into()).or_insert(id);
    Ok((proton.join("proton"), vec!["waitforexitandrun".into()], false))
}

const NO_PROTON: &str = "this is a Windows game: install Proton in Steam (any game's Properties, Compatibility) or umu-launcher";

/// The full command line of one game copy. `pad_paths` are the device paths of the seat's controllers, which the
/// bwrap sandbox keeps (a holder already has them).
pub fn copy_line(spec: &LaunchSpec, seat: &LinuxSeat, tools: &Tools, pad_paths: &[String], sandbox: Sandbox) -> Result<Line, String> {
    let opts = &seat.opts;
    let exe = spec.exe.display().to_string();
    let mut env = spec.env.clone();
    env.extend(opts.env.clone());
    // A seat with its own Steam account: the copy runs with that Steam's home, so it talks to that Steam.
    let steam_home = spec.run_as.as_ref().and_then(couchlink_steam::accounts_home);

    // Innermost: the game.
    let mut line: Vec<String> = Vec::new();
    let mut umu = false;
    if opts.uses_proton(&exe) {
        let app_id = env.get("SteamAppId").and_then(|a| a.parse().ok());
        let (program, args, through_umu) = proton_command(tools, &opts.proton, &seat.game_id, app_id, &seat.data_dir, &mut env)?;
        line.push(program.display().to_string());
        line.extend(args);
        umu = through_umu;
        if let Some(home) = &steam_home {
            // umu keeps its runtime under the user's data folder, shared by every seat.
            let data = std::env::var("XDG_DATA_HOME").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
            env.insert("XDG_DATA_HOME".into(), data);
            env.insert("HOME".into(), home.display().to_string());
        }
    } else if let Some(home) = &steam_home {
        env.insert("HOME".into(), home.display().to_string());
    } else {
        // A native game keeps its settings and saves in a home of the seat's own.
        let home = seat.data_dir.join("home");
        env.insert("HOME".into(), home.display().to_string());
        for (var, sub) in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_STATE_HOME", ".local/state"), ("XDG_CACHE_HOME", ".cache")] {
            env.insert(var.into(), home.join(sub).display().to_string());
        }
    }
    line.push(exe);
    line.extend(spec.args.iter().cloned());

    // In Game Mode every copy's window goes into the session's nested KWin (`gamemode_linux.rs`).
    let nested = super::gamemode_linux::active();
    if let Some(n) = &nested {
        env.insert("DISPLAY".into(), n.display.clone());
        if !n.wayland.is_empty() {
            env.insert("WAYLAND_DISPLAY".into(), n.wayland.clone());
        }
    }

    // Around it: the seat's own window.
    let mut display_file = None;
    match (&tools.gamescope, opts.gamescope) {
        (Some(gamescope), true) => {
            let (w, h) = (seat.rect.w.max(1).to_string(), seat.rect.h.max(1).to_string());
            // A nested gamescope (SDL backend) in a window of the seat's size, the game rendered at that size.
            let mut g = vec![gamescope.display().to_string(), "--backend".into(), "sdl".into(), "-W".into(), w.clone(), "-H".into(), h.clone(), "-w".into(), w, "-h".into(), h];
            if seat.borderless {
                g.push("-b".into());
            }
            if let Some(fps) = seat.fps_limit {
                g.extend(["--framerate-limit".into(), fps.to_string()]);
            }
            g.extend(couchlink_core::recipe::split_args(&opts.gamescope_args));
            // The game first tells the daemon which display gamescope gave it, for the seat's keyboard and mouse.
            let file = seat.data_dir.join("display");
            g.extend(["--".into(), "sh".into(), "-c".into(), "printf %s \"$DISPLAY\" > \"$0\"; exec \"$@\"".into(), file.display().to_string()]);
            g.extend(line);
            line = g;
            display_file = Some(file);
            // gamescope's own window goes through X11 (XWayland on a Wayland desktop), where couchlinkd can place it.
            if std::env::var_os("DISPLAY").is_some() || nested.is_some() {
                env.insert("SDL_VIDEODRIVER".into(), "x11".into());
            }
            // gamescope's Vulkan layer is for games under a gamescope session, and makes nested ones unstable.
            env.insert("ENABLE_GAMESCOPE_WSI".into(), "0".into());
        }
        (None, true) => eprintln!("couchlinkd: gamescope is not installed; seat {:?}'s own window is placed instead", spec.seat),
        _ => {}
    }

    // Outermost: the input sandbox.
    match sandbox {
        Sandbox::Holder(pid) => {
            // The copy runs inside the holder's namespace, where the copy folder is where the game expects it.
            let fix = |s: &String| super::ns_linux::inside(s, pid);
            let program = PathBuf::from(fix(&line[0]));
            let args = line[1..].iter().map(fix).collect();
            let env = env.iter().map(|(k, v)| (k.clone(), fix(v))).collect();
            Ok(Line { program, args, env, umu, display_file })
        }
        Sandbox::Bwrap => {
            let bwrap = tools.bwrap.clone().ok_or(
                "this system does not let Couchlink make a sandbox of its own (user namespaces are off) and bubblewrap (bwrap) is not \
                 installed; without one every copy would see every controller",
            )?;
            let mut args = seat_args(pad_paths, &[]);
            args.extend(line);
            Ok(Line { program: bwrap, args, env, umu, display_file })
        }
    }
}

/// What to say when Proton's runtime cannot make its sandbox inside the seat's.
fn apparmor(e: &str) -> String {
    format!(
        "Proton cannot start inside Couchlink's sandbox on this system ({e}). On Ubuntu 24.04 and later AppArmor blocks a sandbox \
         inside another one; `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0` lifts that until the next restart"
    )
}

/// The same check for copies sandboxed with bwrap, once per daemon.
fn nested_sandbox(bwrap: &Path) -> Result<(), String> {
    static RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    RESULT.get_or_init(|| try_nested(bwrap)).clone()
}

fn try_nested(bwrap: &Path) -> Result<(), String> {
    let b = bwrap.display().to_string();
    let out = Command::new(bwrap)
        .args(["--dev-bind", "/", "/", "--die-with-parent", "--", &b, "--dev-bind", "/", "/", "--", "true"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run bwrap: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(apparmor(String::from_utf8_lossy(&out.stderr).trim()))
}

/// Steam's own Proton for `choice` (a folder name or path), or the newest installed one when there is no choice.
fn steam_proton(choice: &str) -> Option<PathBuf> {
    let choice = choice.trim();
    if Path::new(choice).is_absolute() {
        return Some(PathBuf::from(choice)).filter(|p| p.join("proton").is_file());
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in proton_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let wanted = if choice.is_empty() || choice == "GE-Proton" || choice == "UMU-Proton" { name.contains("Proton") } else { name == choice };
            if wanted && e.path().join("proton").is_file() {
                found.push(e.path());
            }
        }
    }
    found.into_iter().max_by_key(|p| proton_rank(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

/// Orders Proton folders: numbered releases by version, Experimental just above them.
fn proton_rank(name: &str) -> (u32, Vec<u32>) {
    let numbers: Vec<u32> = name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
    (if name.contains("Experimental") { 1 } else { 0 }, numbers)
}

/// What PROTONPATH becomes for the recipe's `proton`: none (umu's own), umu's `GE-Proton` keyword, or a folder.
pub fn proton_path(choice: &str) -> Result<Option<String>, String> {
    let choice = choice.trim();
    if choice.is_empty() {
        return Ok(None);
    }
    if choice == "GE-Proton" || choice == "UMU-Proton" {
        return Ok(Some(choice.to_string()));
    }
    let given = Path::new(choice);
    if given.is_absolute() {
        return if given.join("proton").is_file() { Ok(Some(choice.to_string())) } else { Err(format!("{choice} is not a Proton folder (it has no proton script)")) };
    }
    proton_dirs()
        .into_iter()
        .map(|d| d.join(choice))
        .find(|d| d.join("proton").is_file())
        .map(|d| Some(d.display().to_string()))
        .ok_or_else(|| format!("Proton \"{choice}\" is not installed (looked in Steam's compatibilitytools.d and steamapps/common)"))
}

/// Folders that hold Proton builds: Steam's `compatibilitytools.d` and every library's `steamapps/common`.
fn proton_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for steam in [home.join(".steam/root"), home.join(".steam/steam"), home.join(".local/share/Steam")] {
            out.push(steam.join("compatibilitytools.d"));
        }
    }
    out.extend(couchlink_steam::steam_libraries().into_iter().map(|l| l.join("steamapps").join("common")));
    out
}

/// A process started in a process group of its own; while it lives, its seat's keyboards and mice are its own.
struct Group {
    child: std::process::Child,
    _kbm: Option<super::rawkbm::linux::XGuard>,
}

impl Child for Group {
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
        // SAFETY: a signal to the process group this child leads.
        unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use couchlink_core::recipe_linux::{LinuxOpts, Runner};
    use couchlink_core::session::Rect;

    fn spec(exe: &str) -> LaunchSpec {
        LaunchSpec {
            seat: Some(1),
            exe: exe.into(),
            args: vec!["-windowed".into()],
            env: BTreeMap::from([("SteamAppId".to_string(), "264710".to_string())]),
            cwd: "/games/x".into(),
            game_dir: "/state/instances/x/1".into(),
            devices: vec![],
            locks: vec![],
            input: Default::default(),
            saves: vec![],
            hook_children: false,
            run_as: None,
            linux: None,
            log: None,
        }
    }

    fn seat(opts: LinuxOpts) -> LinuxSeat {
        LinuxSeat {
            opts,
            game_id: "umu-264710".into(),
            rect: Rect { x: 960, y: 0, w: 960, h: 1080 },
            borderless: true,
            fps_limit: None,
            data_dir: "/state/instances/x/.seats/1".into(),
            sandbox_pid: None,
        }
    }

    fn tools() -> Tools {
        Tools { bwrap: Some("/usr/bin/bwrap".into()), gamescope: Some("/usr/bin/gamescope".into()), umu: Some("/usr/bin/umu-run".into()) }
    }

    /// The part of the line after bwrap's own options.
    fn after_sandbox(l: &Line) -> Vec<String> {
        let i = l.args.iter().position(|a| a == "--").unwrap();
        l.args[i + 1..].to_vec()
    }

    const SH: [&str; 3] = ["sh", "-c", "printf %s \"$DISPLAY\" > \"$0\"; exec \"$@\""];

    #[test]
    fn a_windows_game_runs_through_proton_in_gamescope_in_the_sandbox() {
        let l = copy_line(&spec("/state/instances/x/1/Game.exe"), &seat(LinuxOpts::default()), &tools(), &[], Sandbox::Bwrap).unwrap();
        assert_eq!(l.program, PathBuf::from("/usr/bin/bwrap"));
        let mut want: Vec<&str> = vec!["/usr/bin/gamescope", "--backend", "sdl", "-W", "960", "-H", "1080", "-w", "960", "-h", "1080", "-b", "--"];
        want.extend(SH);
        want.extend(["/state/instances/x/.seats/1/display", "/usr/bin/umu-run", "/state/instances/x/1/Game.exe", "-windowed"]);
        assert_eq!(after_sandbox(&l), want);
        assert!(l.umu);
        assert_eq!(l.display_file, Some(PathBuf::from("/state/instances/x/.seats/1/display")));
        assert_eq!(l.env["WINEPREFIX"], "/state/instances/x/.seats/1/prefix", "a prefix of the seat's own");
        assert_eq!(l.env["GAMEID"], "umu-264710");
        assert_eq!(l.env["PROTON_ENABLE_HIDRAW"], "1");
        assert_eq!(l.env["ENABLE_GAMESCOPE_WSI"], "0");
        assert_eq!(l.env["SteamAppId"], "264710", "the recipe's environment is kept");
        assert!(!l.env.contains_key("PROTONPATH") && !l.env.contains_key("HOME"));
    }

    #[test]
    fn gamescope_holds_the_copy_to_the_fps_limit() {
        let s = LinuxSeat { fps_limit: Some(30), ..seat(LinuxOpts::default()) };
        let l = copy_line(&spec("/g/Game.exe"), &s, &tools(), &[], Sandbox::Bwrap).unwrap();
        assert!(after_sandbox(&l).windows(2).any(|w| w == ["--framerate-limit", "30"]), "{:?}", l.args);
        let l = copy_line(&spec("/g/Game.exe"), &seat(LinuxOpts::default()), &tools(), &[], Sandbox::Bwrap).unwrap();
        assert!(!l.args.iter().any(|a| a == "--framerate-limit"));
    }

    #[test]
    fn in_its_holder_a_copy_sees_paths_as_inside() {
        let s = LinuxSeat { sandbox_pid: Some(77), ..seat(LinuxOpts::default()) };
        let sp = LaunchSpec { exe: "/proc/77/root/state/instances/x/1/Game.exe".into(), args: vec!["--dir=/proc/77/root/state/instances/x/1".into()], ..spec("") };
        let l = copy_line(&sp, &s, &tools(), &[], Sandbox::Holder(77)).unwrap();
        assert_eq!(l.program, PathBuf::from("/usr/bin/gamescope"), "no bwrap: the copy joins its holder");
        assert!(l.args.iter().any(|a| a == "/state/instances/x/1/Game.exe"), "{:?}", l.args);
        assert!(l.args.iter().any(|a| a == "--dir=/state/instances/x/1"));
        assert!(!l.args.iter().chain(l.env.values()).any(|a| a.contains("/proc/77/root")));
        let l = copy_line(&sp, &s, &Tools { bwrap: None, ..tools() }, &[], Sandbox::Holder(77)).unwrap();
        assert_eq!(l.program, PathBuf::from("/usr/bin/gamescope"), "bwrap is not needed then");
    }

    #[test]
    fn a_native_game_gets_a_home_of_its_own() {
        let opts = LinuxOpts { gamescope_args: "--force-grab-cursor".into(), env: BTreeMap::from([("A".to_string(), "1".to_string())]), ..LinuxOpts::default() };
        let l = copy_line(&spec("/state/instances/x/1/game.x86_64"), &seat(opts), &tools(), &[], Sandbox::Bwrap).unwrap();
        let rest = after_sandbox(&l);
        assert!(rest.contains(&"--force-grab-cursor".to_string()));
        assert_eq!(rest[rest.len() - 2..], ["/state/instances/x/1/game.x86_64", "-windowed"]);
        assert_eq!(l.env["HOME"], "/state/instances/x/.seats/1/home");
        assert_eq!(l.env["XDG_DATA_HOME"], "/state/instances/x/.seats/1/home/.local/share");
        assert_eq!(l.env["A"], "1");
        assert!(!l.env.contains_key("WINEPREFIX") && !l.umu);
    }

    #[test]
    fn without_gamescope_the_game_runs_straight_in_the_sandbox() {
        let opts = LinuxOpts { gamescope: false, runner: Runner::Native, ..LinuxOpts::default() };
        let l = copy_line(&spec("/g/run.sh"), &seat(opts), &tools(), &[], Sandbox::Bwrap).unwrap();
        assert_eq!(after_sandbox(&l), ["/g/run.sh", "-windowed"]);
        assert_eq!(l.display_file, None);
        // Not installed is the same, with a note in the log.
        let l = copy_line(&spec("/g/run.sh"), &seat(LinuxOpts::default()), &Tools { gamescope: None, ..tools() }, &[], Sandbox::Bwrap).unwrap();
        assert_eq!(after_sandbox(&l), ["/g/run.sh", "-windowed"]);
    }

    #[test]
    fn missing_tools_are_named() {
        let s = seat(LinuxOpts::default());
        let e = copy_line(&spec("/g/Game.exe"), &s, &Tools { bwrap: None, ..tools() }, &[], Sandbox::Bwrap).unwrap_err();
        assert!(e.contains("bwrap"), "{e}");
        assert!(copy_line(&spec("/g/game.x86_64"), &s, &Tools { umu: None, ..tools() }, &[], Sandbox::Bwrap).is_ok(), "native games need no Proton");
    }

    #[test]
    fn steams_own_proton_is_found_and_ranked() {
        let root = std::env::temp_dir().join(format!("couchlink-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p8 = root.join("Proton 8.0");
        std::fs::create_dir_all(&p8).unwrap();
        std::fs::write(p8.join("proton"), "#!/bin/sh\n").unwrap();
        assert!(proton_rank("Proton 9.0") > proton_rank("Proton 8.0"));
        assert!(proton_rank("Proton - Experimental") > proton_rank("Proton 9.0"));
        assert!(proton_rank("GE-Proton10-3") > proton_rank("GE-Proton9-20"));
        assert_eq!(steam_proton(&p8.display().to_string()), Some(p8.clone()), "a path is taken as it is");
        assert_eq!(steam_proton(&root.join("nothing").display().to_string()), None);
        assert_eq!(steam_proton("No-Such-Proton-123"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CI allows nested sandboxes, so there the check must pass (COUCHLINK_REQUIRE_SANDBOX_TEST=1).
    #[test]
    fn a_sandbox_inside_the_sandbox_is_tried() {
        let Some(bwrap) = which("bwrap") else {
            assert!(!std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1"), "bwrap is missing");
            return;
        };
        let r = try_nested(&bwrap);
        if std::env::var("COUCHLINK_REQUIRE_SANDBOX_TEST").is_ok_and(|v| v == "1") {
            assert_eq!(r, Ok(()));
        } else if let Err(e) = r {
            assert!(e.contains("AppArmor"), "{e}");
        }
        assert!(try_nested(Path::new("/nonexistent/bwrap")).unwrap_err().contains("cannot run bwrap"));
    }

    #[test]
    fn proton_choices() {
        assert_eq!(proton_path(""), Ok(None));
        assert_eq!(proton_path("GE-Proton"), Ok(Some("GE-Proton".into())));
        assert!(proton_path("/nowhere/Proton-X").unwrap_err().contains("not a Proton folder"));
        assert!(proton_path("No-Such-Proton-123").unwrap_err().contains("not installed"));
        let dir = std::env::temp_dir().join(format!("couchlink-proton-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proton"), "#!/bin/sh\n").unwrap();
        assert_eq!(proton_path(&dir.display().to_string()), Ok(Some(dir.display().to_string())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helpers_run_plain_or_through_proton() {
        let l = helper_line(&LaunchSpec { seat: None, ..spec("/g/server.sh") }, &tools()).unwrap();
        assert_eq!((l.program, l.args), (PathBuf::from("/g/server.sh"), vec!["-windowed".to_string()]));
        let l = helper_line(&LaunchSpec { seat: None, ..spec("/g/Server.exe") }, &tools()).unwrap();
        assert_eq!(l.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(l.args[0], "/g/Server.exe");
        assert_eq!(l.env["WINEPREFIX"], "/state/instances/x/1/.couchlink-prefix/prefix");
    }

    #[test]
    fn a_process_group_is_killed_whole() {
        // sh starts a sleep of its own; killing the group ends both.
        let pidfile = std::env::temp_dir().join(format!("couchlink-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "sh", &pidfile.display().to_string()]).process_group(0);
        let mut g = Group { child: cmd.spawn().unwrap(), _kbm: None };
        let t = std::time::Instant::now();
        while !pidfile.exists() {
            assert!(t.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let grandchild: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert_eq!(g.try_wait(), Ok(None));
        g.kill();
        let _ = g.child.wait();
        let t = std::time::Instant::now();
        while super::super::window::process_alive(grandchild as u32) {
            assert!(t.elapsed().as_secs() < 5, "the copy's own child survived");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(&pidfile);
    }
}
