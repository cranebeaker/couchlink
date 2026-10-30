//! Couchlink Steam layer. See docs/launch.md ("The Steam ladder", "Interfaces between parts").
//!
//! * `library`: find Steam, its libraries and installed apps (read-only).
//! * `goldberg`: pinned gbe_fork download, hash check, extraction (LGPL-3, separate unmodified DLLs), including
//!   the experimental steamclient set when asked for.
//! * `prepare`: per-seat emulator setup inside a hardlinked game copy, plus `Undo` records.
//! * `options`: typed gbe_fork settings a recipe may ask for (`GoldbergOptions`) and the ini text they make.
//! * `interfaces`: `steam_interfaces.txt` from the original `steam_api(64).dll` (port of gbe's generate_interfaces).
//! * `dlc`: the DLC list. Only DLC the player already has installed is ever reported.
//! * `steamclient`: ColdClientLoader mode (`ColdClientLoader.ini`, the loader as the thing to launch).
//! * `accounts`: opt-in second real Steam account (plan text and guarded code paths).
//! * `vdf`, `pe`: the small parsers behind them.

pub mod accounts;
pub mod dlc;
pub mod error;
pub mod goldberg;
pub mod goldberg_linux;
pub mod interfaces;
pub mod library;
pub mod options;
pub mod pe;
pub mod prepare;
pub mod steamclient;
pub mod vdf;

pub use accounts::{
    AccountsConfig, LaunchedAs, Step, accounts_apply, accounts_home, accounts_launch, accounts_plan, accounts_ready, accounts_remove,
    accounts_start_steam, accounts_steam_running, merge_env, seat_config,
};
pub use dlc::{DlcEntry, DlcSource, dlc_entries, installed_dlc_ids};
pub use error::{Result, SteamError};
pub use goldberg::{
    GOLDBERG, GoldbergFiles, GoldbergPin, SteamClientFiles, ensure_goldberg, ensure_goldberg_steamclient,
};
pub use interfaces::{generate_interfaces, original_needs_interfaces_file, scan_interfaces};
pub use library::{InstalledApp, find_app, installed_apps, steam_libraries, steam_root};
pub use options::{GoldbergOptions, InterfacesMode, SeatSelector};
pub use pe::{Bitness, pe_bitness};
pub use prepare::{
    PrepareOptions, Prepared, SeatSteam, SteamMode, Undo, party_listen_port, prepare, prepare_full, prepare_with,
    seat_steam_id, undo, undo_all,
};
pub use steamclient::{GameLaunch, LoaderLaunch};
