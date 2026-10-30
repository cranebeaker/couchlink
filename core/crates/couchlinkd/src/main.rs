//! couchlinkd: the Couchlink background daemon.

mod accounts;
mod catalog;
mod displays;
mod hub;
mod profiles;
mod sdl;
mod session;
mod ws;

use couchlink_core::paths::{self, DaemonInfo};
use couchlink_core::PROTOCOL_VERSION;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

const EXIT_ALREADY_RUNNING: i32 = 3;

struct Args {
    dev: bool,
    stay: bool,
    virtual_pads: u32,
    recipes: Option<std::path::PathBuf>,
}

fn usage() {
    println!(
        "couchlinkd {}\n\n\
         usage: couchlinkd [--dev] [--virtual-pads N] [--stay] [--recipes DIR]\n\n\
         --dev             attach virtual gamepads and enable debug_press (for tests)\n\
         --virtual-pads N  number of virtual pads in --dev mode (default 2)\n\
         --stay            do not exit when the last client disconnects\n\
         --help, --version\n\n\
         env: COUCHLINK_STATE_DIR overrides the folder that holds daemon.json and catalog.json\n\
         \x20    COUCHLINK_CATALOG_URL, COUCHLINK_STATS_URL override where the game catalog comes from",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { dev: false, stay: false, virtual_pads: 2, recipes: None };
    let mut pads_given = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dev" => args.dev = true,
            "--stay" => args.stay = true,
            "--virtual-pads" => {
                let v = it.next().ok_or("--virtual-pads needs a number")?;
                args.virtual_pads = v.parse().map_err(|_| format!("bad --virtual-pads value {v}"))?;
                if args.virtual_pads > 8 {
                    return Err("--virtual-pads max is 8".into());
                }
                pads_given = true;
            }
            "--recipes" => args.recipes = Some(it.next().ok_or("--recipes needs a folder")?.into()),
            "--help" | "-h" => {
                usage();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("couchlinkd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if pads_given && !args.dev {
        return Err("--virtual-pads needs --dev".into());
    }
    if !args.dev {
        args.virtual_pads = 0;
    }
    Ok(args)
}

/// True when another daemon for the same state folder runs. A named mutex settles two daemons that start in the
/// same moment (two app windows opened at once); the port check covers a daemon from an older build.
fn already_running() -> bool {
    if !claim_single_instance() {
        return true;
    }
    let Some(info) = paths::read_daemon_info() else { return false };
    if info.pid == std::process::id() {
        return false;
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// Takes the per-state-folder mutex for the life of the process; false when another process holds it.
#[cfg(windows)]
fn claim_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let dir = paths::state_dir().display().to_string().to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let name: Vec<u16> = format!("Local\\couchlinkd-{h:016x}").encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated name; the handle is kept open on purpose until the process exits.
    unsafe {
        let m = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !m.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Takes a lock file in `XDG_RUNTIME_DIR` (else the state folder), named after the state folder, for the life of the
/// process; false when another couchlinkd holds it.
#[cfg(target_os = "linux")]
fn claim_single_instance() -> bool {
    use std::os::unix::io::IntoRawFd;
    let state = paths::state_dir();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in state.display().to_string().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|d| d.is_dir()).unwrap_or_else(|| state.clone());
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(format!("couchlinkd-{h:016x}.lock"))) else {
        return true;
    };
    // The descriptor stays open (and locked) until the process exits.
    let fd = file.into_raw_fd();
    // SAFETY: flock on a descriptor this process owns.
    unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn claim_single_instance() -> bool {
    true
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn main() {
    // A seat's sandbox holder (session/ns_linux.rs): a short-lived child of the daemon, not a daemon.
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some(session::ns_linux::HOLDER_ARG) {
        std::process::exit(session::ns_linux::holder_main());
    }
    // The elevated half of setting up or removing a seat's own Steam account (see accounts.rs).
    if let Some(code) = accounts::cli(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(code);
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("couchlinkd: {e}");
            usage();
            std::process::exit(2);
        }
    };
    if already_running() {
        eprintln!("couchlinkd: already running");
        std::process::exit(EXIT_ALREADY_RUNNING);
    }
    match run(args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("couchlinkd: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    // Undo whatever a crashed session left behind before anything else starts.
    let session_cfg = {
        let state_dir = paths::state_dir();
        let cfg = session::Config::new(args.recipes.clone(), state_dir.clone(), session::Deps::standard(&state_dir));
        let report = cfg.recover();
        if report.undone > 0 || !report.failed.is_empty() {
            eprintln!("couchlinkd: undid {} change(s) left by an earlier session; {} could not be undone {:?}", report.undone, report.failed.len(), report.failed);
        }
        cfg
    };
    let (sdl_tx, mut sdl_rx) = unbounded_channel::<sdl::SdlEvent>();
    let sdl = sdl::spawn(args.virtual_pads, sdl_tx)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let pid = std::process::id();
    let mut sdl: Option<sdl::SdlHandle> = rt.block_on(async {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.map_err(|e| format!("cannot listen: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = random_token()?;
        paths::write_daemon_info(&DaemonInfo { port, token: token.clone(), pid, protocol: PROTOCOL_VERSION })
            .map_err(|e| format!("cannot write daemon.json: {e}"))?;
        println!("couchlinkd {} listening on 127.0.0.1:{port}{}", env!("CARGO_PKG_VERSION"), if args.dev { " (dev)" } else { "" });

        let (hub_tx, hub_rx) = unbounded_channel::<hub::HubMsg>();
        let forward = hub_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = sdl_rx.recv().await {
                if forward.send(hub::HubMsg::Sdl(ev)).is_err() {
                    break;
                }
            }
        });
        tokio::spawn(ws::serve(listener, Arc::new(token), hub_tx.clone()));
        // Serve the cache at once; refresh when it is stale, now and every 30 minutes.
        let ticker = hub_tx.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(30 * 60));
            loop {
                every.tick().await;
                if ticker.send(hub::HubMsg::CatalogTick).is_err() {
                    break;
                }
            }
        });

        // Keyboards and mice: the listener reports plug, unplug and press-to-join to the hub. It is optional; a PC
        // where Raw Input cannot start simply has no keyboard seats.
        let kbm_tx = hub_tx.clone();
        session::rawkbm::subscribe(move |ev| {
            let _ = kbm_tx.send(hub::HubMsg::Kbm(ev));
        });
        if let Err(e) = session::rawkbm::start() {
            eprintln!("couchlinkd: keyboard and mouse seats are off: {e}");
        }

        let hub = hub::Hub::new(args.dev, args.stay, sdl, hub_tx, catalog::Urls::from_env(), session_cfg);
        let sdl = tokio::select! {
            sdl = hub.run(hub_rx) => sdl,
            _ = tokio::signal::ctrl_c() => {
                eprintln!("couchlinkd: interrupted");
                return Ok::<_, String>(None);
            }
        };
        Ok(Some(sdl))
    })?;

    paths::remove_daemon_info(pid);
    if let Some(s) = sdl.as_mut() {
        s.shutdown();
    }
    // Ctrl-C drops the hub (and the SDL handle with it); the SDL thread then
    // ends when the process exits.
    Ok(())
}
