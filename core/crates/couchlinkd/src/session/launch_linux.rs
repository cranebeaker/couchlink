//! Starting game copies on Linux (docs/launch.md, "Session pipeline (Linux)"). A copy is, from the outside in:
//!
//! 1. `bwrap`: the input sandbox (`sandbox_linux.rs`), so the copy sees only its own seat's devices,
//! 2. `gamescope`: a window of the seat's size of its own; a copy keeps getting its controller's input while another
//!    copy's window has the focus, because it reads the controller itself,
//! 3. `umu-run` (Proton, for Windows games) with a prefix of the seat's own, or the native program, with a home of
//!    the seat's own.
//!
//! Everything a copy starts is in its own process group, which is what `kill` ends. bwrap's `--die-with-parent`
//! ends the copy when couchlinkd dies.

use super::launch::{Child, LaunchSpec, Launched, Launcher, LinuxSeat};
use super::sandbox_linux::{seat_args, which};
use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct LinuxLauncher;

impl Launcher for LinuxLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Launched, String> {
        if spec.run_as.is_some() {
            return Err("an own Steam account per player is only available on Windows for now".to_string());
        }
        let tools = Tools::find();
        let line = match &spec.linux {
            Some(seat) => {
                let home = seat.data_dir.join("home");
                std::fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
                copy_line(spec, seat, &tools, &super::hook::pad_nodes(&spec.devices))?
            }
            None => helper_line(spec, &tools)?,
        };
        let mut cmd = Command::new(&line.program);
        cmd.args(&line.args).envs(&line.env).current_dir(&spec.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", line.program.display()))?;
        Ok(Launched { child: Box::new(Group { child }), hooked: false })
    }
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
}

/// What a helper (`launch.first`, per-seat helpers) runs as: Windows programs through Proton in the game's shared
/// prefix, others as they are. Helpers are not sandboxed and get no gamescope window.
fn helper_line(spec: &LaunchSpec, tools: &Tools) -> Result<Line, String> {
    let exe = spec.exe.display().to_string();
    let mut env = spec.env.clone();
    if !couchlink_core::recipe_linux::LinuxOpts::default().uses_proton(&exe) {
        return Ok(Line { program: spec.exe.clone(), args: spec.args.clone(), env });
    }
    let umu = tools.umu.clone().ok_or(NO_UMU)?;
    env.entry("GAMEID".into()).or_insert_with(|| "umu-0".into());
    env.entry("WINEPREFIX".into()).or_insert_with(|| spec.game_dir.join(".couchlink-prefix").display().to_string());
    let mut args = vec![exe];
    args.extend(spec.args.iter().cloned());
    Ok(Line { program: umu, args, env })
}

const NO_UMU: &str = "this is a Windows game, and starting it on Linux needs umu-launcher (umu-run), which is not installed";

/// The full command line of one game copy.
pub fn copy_line(spec: &LaunchSpec, seat: &LinuxSeat, tools: &Tools, pad_paths: &[String]) -> Result<Line, String> {
    let opts = &seat.opts;
    let exe = spec.exe.display().to_string();
    let proton = opts.uses_proton(&exe);
    let mut env = spec.env.clone();
    env.extend(opts.env.clone());

    // Innermost: the game.
    let mut inner: Vec<String> = Vec::new();
    if proton {
        let prefix = seat.data_dir.join("prefix");
        env.insert("WINEPREFIX".into(), prefix.display().to_string());
        env.insert("GAMEID".into(), seat.game_id.clone());
        // Wine's controller driver reads hidraw too; the sandbox has hidden every other seat's hidraw nodes.
        env.entry("PROTON_ENABLE_HIDRAW".into()).or_insert_with(|| "1".into());
        env.entry("STORE".into()).or_insert_with(|| "none".into());
        match proton_path(&opts.proton)? {
            Some(p) => {
                env.insert("PROTONPATH".into(), p);
            }
            None => {
                env.remove("PROTONPATH");
            }
        }
        let umu = tools.umu.clone().ok_or(NO_UMU)?;
        inner.push(umu.display().to_string());
    } else {
        // A native game keeps its settings and saves in a home of the seat's own.
        let home = seat.data_dir.join("home");
        env.insert("HOME".into(), home.display().to_string());
        for (var, sub) in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_STATE_HOME", ".local/state"), ("XDG_CACHE_HOME", ".cache")] {
            env.insert(var.into(), home.join(sub).display().to_string());
        }
    }
    inner.push(exe);
    inner.extend(spec.args.iter().cloned());

    // Around it: the seat's own window.
    let mut line = inner;
    if opts.gamescope {
        let gamescope = tools.gamescope.clone().ok_or("gamescope is not installed; Couchlink needs it to give each player a window of their own on Linux")?;
        let (w, h) = (seat.rect.w.max(1).to_string(), seat.rect.h.max(1).to_string());
        // A nested gamescope (SDL backend) in a window of the seat's size, the game rendered at that size.
        let mut g = vec![gamescope.display().to_string(), "--backend".into(), "sdl".into(), "-W".into(), w.clone(), "-H".into(), h.clone(), "-w".into(), w, "-h".into(), h];
        if seat.borderless {
            g.push("-b".into());
        }
        g.extend(couchlink_core::recipe::split_args(&opts.gamescope_args));
        g.push("--".into());
        g.extend(line);
        line = g;
        // gamescope's own window goes through X11 (XWayland on a Wayland desktop), where couchlinkd can place it.
        if std::env::var_os("DISPLAY").is_some() {
            env.insert("SDL_VIDEODRIVER".into(), "x11".into());
        }
        // gamescope's Vulkan layer is for games under a gamescope session, and makes nested ones unstable.
        env.insert("ENABLE_GAMESCOPE_WSI".into(), "0".into());
    }

    // Outermost: the input sandbox.
    let bwrap = tools.bwrap.clone().ok_or("bubblewrap (bwrap) is not installed; without it every copy would see every controller")?;
    let mut args = seat_args(pad_paths, &[]);
    args.extend(line);
    Ok(Line { program: bwrap, args, env })
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

/// A process started in a process group of its own.
struct Group {
    child: std::process::Child,
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
        }
    }

    fn seat(opts: LinuxOpts) -> LinuxSeat {
        LinuxSeat { opts, game_id: "umu-264710".into(), rect: Rect { x: 960, y: 0, w: 960, h: 1080 }, borderless: true, data_dir: "/state/instances/x/.seats/1".into() }
    }

    fn tools() -> Tools {
        Tools { bwrap: Some("/usr/bin/bwrap".into()), gamescope: Some("/usr/bin/gamescope".into()), umu: Some("/usr/bin/umu-run".into()) }
    }

    /// The part of the line after bwrap's own options.
    fn after_sandbox(l: &Line) -> Vec<String> {
        let i = l.args.iter().position(|a| a == "--").unwrap();
        l.args[i + 1..].to_vec()
    }

    #[test]
    fn a_windows_game_runs_through_proton_in_gamescope_in_the_sandbox() {
        let l = copy_line(&spec("/state/instances/x/1/Game.exe"), &seat(LinuxOpts::default()), &tools(), &[]).unwrap();
        assert_eq!(l.program, PathBuf::from("/usr/bin/bwrap"));
        assert_eq!(
            after_sandbox(&l),
            ["/usr/bin/gamescope", "--backend", "sdl", "-W", "960", "-H", "1080", "-w", "960", "-h", "1080", "-b", "--", "/usr/bin/umu-run", "/state/instances/x/1/Game.exe", "-windowed"]
        );
        assert_eq!(l.env["WINEPREFIX"], "/state/instances/x/.seats/1/prefix", "a prefix of the seat's own");
        assert_eq!(l.env["GAMEID"], "umu-264710");
        assert_eq!(l.env["PROTON_ENABLE_HIDRAW"], "1");
        assert_eq!(l.env["ENABLE_GAMESCOPE_WSI"], "0");
        assert_eq!(l.env["SteamAppId"], "264710", "the recipe's environment is kept");
        assert!(!l.env.contains_key("PROTONPATH") && !l.env.contains_key("HOME"));
    }

    #[test]
    fn a_native_game_gets_a_home_of_its_own() {
        let opts = LinuxOpts { gamescope_args: "--force-grab-cursor".into(), env: BTreeMap::from([("A".to_string(), "1".to_string())]), ..LinuxOpts::default() };
        let l = copy_line(&spec("/state/instances/x/1/game.x86_64"), &seat(opts), &tools(), &[]).unwrap();
        let rest = after_sandbox(&l);
        assert_eq!(rest[rest.len() - 4..], ["--force-grab-cursor", "--", "/state/instances/x/1/game.x86_64", "-windowed"]);
        assert_eq!(l.env["HOME"], "/state/instances/x/.seats/1/home");
        assert_eq!(l.env["XDG_DATA_HOME"], "/state/instances/x/.seats/1/home/.local/share");
        assert_eq!(l.env["A"], "1");
        assert!(!l.env.contains_key("WINEPREFIX"));
    }

    #[test]
    fn without_gamescope_the_game_runs_straight_in_the_sandbox() {
        let opts = LinuxOpts { gamescope: false, runner: Runner::Native, ..LinuxOpts::default() };
        let l = copy_line(&spec("/g/run.sh"), &seat(opts), &Tools { gamescope: None, ..tools() }, &[]).unwrap();
        assert_eq!(after_sandbox(&l), ["/g/run.sh", "-windowed"]);
    }

    #[test]
    fn missing_tools_are_named() {
        let s = seat(LinuxOpts::default());
        let e = copy_line(&spec("/g/Game.exe"), &s, &Tools { bwrap: None, ..tools() }, &[]).unwrap_err();
        assert!(e.contains("bwrap"), "{e}");
        let e = copy_line(&spec("/g/Game.exe"), &s, &Tools { gamescope: None, ..tools() }, &[]).unwrap_err();
        assert!(e.contains("gamescope"), "{e}");
        let e = copy_line(&spec("/g/Game.exe"), &s, &Tools { umu: None, ..tools() }, &[]).unwrap_err();
        assert!(e.contains("umu-launcher"), "{e}");
        assert!(copy_line(&spec("/g/game.x86_64"), &s, &Tools { umu: None, ..tools() }, &[]).is_ok(), "native games do not need umu");
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
        assert_eq!(l.env["WINEPREFIX"], "/state/instances/x/1/.couchlink-prefix");
    }

    #[test]
    fn a_process_group_is_killed_whole() {
        // sh starts a sleep of its own; killing the group ends both.
        let pidfile = std::env::temp_dir().join(format!("couchlink-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "sh", &pidfile.display().to_string()]).process_group(0);
        let mut g = Group { child: cmd.spawn().unwrap() };
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
