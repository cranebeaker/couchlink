//! Map a parsed SplitScreen.Me handler onto a Couchlink recipe (`docs/launch.md`).
//!
//! The mapping is written from the meaning of each field, learned from the handler documentation
//! (`readme.txt` of Nucleus Co-op). No code is taken from Nucleus: it is GPL-3 and this crate is MIT.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::hub::{HubGame, game_from_value, handler_values};
use crate::js::{ParsedHandler, Val, num_to_string, parse_handler};
use crate::{Error, Result};

/// A recipe made from a handler, plus what a person still has to finish.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    /// The recipe JSON (`schema` 1, `source` `splitscreenme:<id>`).
    pub recipe: Value,
    /// Things the recipe cannot express yet, one line each.
    pub notes: Vec<String>,
    /// 0.0 to 1.0: how likely the recipe works without hand editing. At or above 0.8 the handler
    /// had nothing the recipe cannot say; below 0.5 a person has real work to do.
    pub confidence: f32,
}

/// Turn a handler into a draft recipe.
///
/// `input` is either the handler script itself (`handler.js`, starts with anything but `{`), or a
/// JSON object with the script and optionally the hub metadata:
///
/// ```json
/// { "handler": { "_id": "...", "title": "...", ... },   // an allhandlers row, a handler/{id}
///                                                        // response, or its "Handlers" wrapper
///   "script": "Hub.Handler.Version = 8; Game.ExecutableName = ..." }
/// ```
///
/// With metadata only (no `script`) a low-confidence skeleton is returned.
pub fn to_recipe(input: &str) -> Result<Draft> {
    let trimmed = input.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Err(Error::Empty);
    }
    let (script, meta): (Option<String>, Option<HubGame>) = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let root: Value = serde_json::from_str(trimmed).map_err(|e| Error::Json(e.to_string()))?;
        let script = ["script", "handler_js", "js"]
            .iter()
            .find_map(|k| root.get(*k).and_then(Value::as_str))
            .map(str::to_string);
        let meta_value = root.get("handler").unwrap_or(&root);
        let meta = handler_values(meta_value).into_iter().next().and_then(game_from_value);
        if script.is_none() && meta.is_none() {
            return Err(Error::NotAHandler);
        }
        (script, meta)
    } else {
        (Some(trimmed.to_string()), None)
    };

    let mut b = Builder::new(meta);
    match script {
        Some(s) => b.p = parse_handler(&s),
        None => b.skeleton_notes(),
    }
    let recipe = assemble(&mut b);
    Ok(b.into_draft(recipe))
}

// ---------------------------------------------------------------------------------------------

struct Builder {
    p: ParsedHandler,
    meta: Option<HubGame>,
    notes: Vec<String>,
    penalties: Vec<(&'static str, f32)>,
    cap: f32,
}

/// Names of Nucleus context calls that need no note: Couchlink does them itself or they only
/// matter to Nucleus's own pipeline.
const IGNORED_CALLS: &[&str] = &[
    "Context.GetFolder",
    "Context.Wait",
    "Context.HideTaskBar",
    "Context.SetProtoInputMultipleControllers",
    "Context.FindLineNumberInTextFile",
    "Context.BackupFile",
    "Context.ModifiedDate",
    "Context.CreatedDate",
    "System.IO.Path.Combine",
    "System.IO.File.Exists",
    "System.IO.Directory.Exists",
    "System.IO.File.GetAttributes",
    "System.IO.File.SetAttributes",
    "Game.AddOption",
    "Nucleus.IniSaveInfo",
    "Math.floor",
    "Math.round",
    "Math.random",
    "Math.ceil",
    "parseInt",
    "parseFloat",
    "String",
    "Number",
];

/// Game fields that, when set only under a condition inside `Game.Play`, make the recipe value
/// a guess.
const RUNTIME_KEYS: &[&str] = &[
    "HookFocus",
    "FakeFocus",
    "FakeFocusInterval",
    "SendFakeFocusMsg",
    "SupportsMultipleKeyboardsAndMice",
    "SupportsKeyboard",
    "SetForegroundWindowElsewhere",
    "StartArguments",
    "KillMutex",
    "UseGoldberg",
    "ExecutableName",
    "MaxPlayers",
    "SymlinkGame",
    "Hook.XInputEnabled",
    "Hook.DInputEnabled",
    "Hook.ForceFocus",
    "SetWindowHook",
    "BinariesFolder",
];

/// Truthy top-level fields with no recipe equivalent: (field, note, penalty).
const UNSUPPORTED_FLAGS: &[(&str, &str, f32)] = &[
    ("RequiresAdmin", "Handler asks for administrator rights; Couchlink runs as the user, so the game may need to be started elevated by hand.", 0.05),
    ("ThirdPartyLaunch", "The game is started by an outside program (a mod launcher), not by Couchlink; the recipe launches the exe directly.", 0.25),
    ("ForceProcessPick", "Handler asks the user to pick the game process by hand after each launch.", 0.05),
    ("PromptBetweenInstances", "Handler pauses and asks the user before opening each next instance.", 0.03),
    ("PromptAfterFirstInstance", "Handler pauses after the first instance for manual steps (host a game, join a server).", 0.03),
    ("LaunchAsDifferentUsers", "Handler runs each instance under its own Windows user; Couchlink does not create Windows accounts.", 0.2),
    ("UseForceBindIP", "Handler binds each instance to its own local IP (ForceBindIP); not modelled.", 0.15),
    ("ChangeIPPerInstance", "Handler gives each instance its own local IP; not modelled.", 0.15),
    ("UseEACBypass", "Handler bypasses Easy Anti-Cheat; Couchlink will not.", 0.2),
    ("UseSteamStubDRMPatcher", "Handler patches Steam stub DRM out of the exe (SteamStub); Couchlink does not patch executables.", 0.15),
    ("UseNemirtingasEpicEmu", "Handler emulates the Epic Online Services with Nemirtingas; not modelled, the Epic login of each copy needs a manual answer.", 0.15),
    ("UseNemirtingasGalaxyEmu", "Handler emulates GOG Galaxy with Nemirtingas; not modelled.", 0.15),
    ("WriteToProcessMemory", "Handler writes into the game's memory after launch; not modelled.", 0.15),
    ("FlawlessWidescreen", "Handler drives Flawless Widescreen; not modelled.", 0.05),
    ("UseDirectX9Wrapper", "Handler installs a DirectX 9 wrapper dll; not modelled.", 0.1),
    ("HardcopyGame", "Handler makes full copies of the game folder; the recipe uses hardlinks instead.", 0.0),
    ("CMDLaunch", "Handler launches through a command line; not modelled.", 0.1),
    ("ProcessChangesAtEnd", "Handler applies window and hook changes only after all instances started.", 0.0),
    ("HideDesktop", "Handler hides the desktop and taskbar while playing; not modelled.", 0.0),
    ("LockMouse", "Handler locks the mouse to the game window; not modelled.", 0.0),
];

/// Truthy list fields with no recipe equivalent.
const UNSUPPORTED_LISTS: &[(&str, &str, f32)] = &[
    ("HexEditExe", "Handler hex-edits the exe", 0.15),
    ("HexEditAllExes", "Handler hex-edits every exe", 0.15),
    ("HexEditFile", "Handler hex-edits files", 0.15),
    ("HexEditAllFiles", "Handler hex-edits all files", 0.15),
    ("CMDBatchBefore", "Handler runs commands before launch", 0.1),
    ("CMDBatchAfter", "Handler runs commands after launch", 0.1),
    ("CMDBatchClose", "Handler runs commands at close", 0.05),
    ("CMDOptions", "Handler passes command line options through cmd", 0.05),
    ("RenameAndOrMoveFiles", "Handler renames or moves game files", 0.1),
    ("DeleteFiles", "Handler deletes game files before launch", 0.1),
    ("SymlinkFoldersTo", "Handler symlinks game folders to other places", 0.1),
    ("CopyFoldersTo", "Handler copies game folders to other places", 0.1),
    ("HardlinkFoldersTo", "Handler hardlinks game folders to other places", 0.1),
    ("CopyCustomUtils", "Handler copies extra utility files into each instance", 0.1),
    ("CustomSteamApiDllPath", "Handler points at custom steam_api dll locations", 0.05),
    ("KillMutexLauncher", "Handler also kills launcher mutexes", 0.05),
    ("KillMutexProcess", "Handler kills mutexes inside a helper process", 0.1),
    ("BackupFiles", "Handler backs up files and restores them at the end", 0.0),
    ("BackupFolders", "Handler backs up folders and restores them at the end", 0.0),
];

impl Builder {
    fn new(meta: Option<HubGame>) -> Self {
        Builder { p: ParsedHandler::default(), meta, notes: Vec::new(), penalties: Vec::new(), cap: 1.0 }
    }

    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.notes.contains(&s) {
            self.notes.push(s);
        }
    }

    /// Lower confidence once per category.
    fn penalize(&mut self, category: &'static str, amount: f32) {
        if amount > 0.0 && !self.penalties.iter().any(|(c, _)| *c == category) {
            self.penalties.push((category, amount));
        }
    }

    fn confidence(&self) -> f32 {
        let total: f32 = self.penalties.iter().map(|(_, a)| a).sum();
        let c = (1.0 - total).min(self.cap).clamp(0.05, 1.0);
        (c * 100.0).round() / 100.0
    }

    fn into_draft(self, mut recipe: Value) -> Draft {
        brace_vars(&mut recipe);
        let confidence = self.confidence();
        Draft { recipe, notes: self.notes, confidence }
    }

    fn skeleton_notes(&mut self) {
        self.cap = 0.15;
        self.note("Only hub metadata was given, not the handler script: exe name, Steam app id, mutexes, files and input flags are unknown.");
        self.note("Download the package from the hub (HubGame::package_url), unzip handler.js, and import again with the script.");
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn norm_path(s: &str) -> String {
    s.replace('\\', "/").trim_matches('/').to_string()
}

fn ref_template(r: &str) -> Option<&'static str> {
    Some(match r {
        "Context.Width" => "$WIDTH",
        "Context.Height" => "$HEIGHT",
        "Context.PosX" => "$X",
        "Context.PosY" => "$Y",
        "Context.Nickname" => "$PLAYER",
        "Context.PlayerID" => "$SEAT",
        "Context.PlayerSteamID" => "$STEAMID",
        "Context.RootInstallFolder" | "Context.RootFolder" => "$GAMEDIR",
        _ => return None,
    })
}

/// Render a value as a recipe template string. `Err` carries the rendering with the parts that
/// have no recipe variable left out.
fn to_template(v: &Val) -> std::result::Result<String, String> {
    fn part(v: &Val, out: &mut String, ok: &mut bool) {
        match v {
            Val::Str(s) => out.push_str(s),
            Val::Num(n) => out.push_str(&num_to_string(*n)),
            Val::Bool(b) => out.push_str(&b.to_string()),
            Val::Ref(r) => match ref_template(r) {
                Some(t) => out.push_str(t),
                None => *ok = false,
            },
            Val::Concat(parts) => parts.iter().for_each(|p| part(p, out, ok)),
            _ => *ok = false,
        }
    }
    let mut out = String::new();
    let mut ok = true;
    part(v, &mut out, &mut ok);
    if ok { Ok(out) } else { Err(out) }
}

fn tail_literal(v: &Val) -> String {
    match v {
        Val::Concat(parts) => match parts.last() {
            Some(Val::Str(s)) => norm_path(s),
            _ => v.display(),
        },
        Val::Str(s) => norm_path(s),
        other => other.display(),
    }
}

fn is_steam_bookkeeping(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("steam_api")
        || n == "steam_appid.txt"
        || n == "steam_interfaces.txt"
        || n.starts_with("steamclient")
        || n == "coldclientloader.ini"
        || n == "coldloader.dll"
        || n == "local_save.txt"
        || n == "steam_settings"
        || n == "settings"
}

/// Files a handler writes only to set up Goldberg; the Steam layer makes these per copy.
fn is_goldberg_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    p.contains("steam_settings/")
        || matches!(
            name,
            "steam_appid.txt"
                | "steam_interfaces.txt"
                | "dlc.txt"
                | "disable_overlay.txt"
                | "account_name.txt"
                | "user_steam_id.txt"
                | "language.txt"
                | "force_account_name.txt"
                | "force_steamid.txt"
                | "force_language.txt"
                | "local_save.txt"
                | "listen_port.txt"
                | "custom_broadcasts.txt"
        )
        || name.starts_with("configs.")
}

fn is_input_shim(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("xinput") || n == "dinput8.dll" || n == "dinput.dll" || n.starts_with("xinputplus")
}

impl Builder {
    fn flag(&self, key: &str) -> bool {
        self.p.flag(key).unwrap_or(false)
    }

    fn any_flag(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.flag(k))
    }
}

fn assemble(b: &mut Builder) -> Value {
    let mut recipe = Map::new();
    recipe.insert("schema".into(), json!(1));

    // identity ------------------------------------------------------------------------------
    let hub_id = b
        .p
        .hub
        .get("Handler.Id")
        .and_then(Val::as_str)
        .map(str::to_string)
        .or_else(|| b.meta.as_ref().map(|m| m.id.clone()));
    let hub_version = b
        .p
        .hub
        .get("Handler.Version")
        .and_then(Val::as_f64)
        .map(|n| n as u64)
        .or_else(|| b.meta.as_ref().map(|m| m.handler_version as u64));
    let game_name = b
        .p
        .text("GameName")
        .filter(|s| !s.is_empty())
        .or_else(|| b.meta.as_ref().map(|m| m.game_name.clone()).filter(|s| !s.is_empty()))
        .or_else(|| b.p.text("GUID"))
        .unwrap_or_else(|| "Unknown game".to_string());
    let id_tail = hub_id.as_deref().map(|i| i.chars().take(6).collect::<String>().to_ascii_lowercase());
    let rid = match &id_tail {
        Some(t) => format!("splitscreenme-{}-{}", short_slug(&game_name, 64 - 15 - t.len()), t),
        None => format!("splitscreenme-{}", short_slug(&game_name, 64 - 14)),
    };
    recipe.insert("id".into(), json!(rid));
    recipe.insert("game".into(), json!(game_name));
    match &hub_id {
        Some(i) => {
            recipe.insert("source".into(), json!(format!("splitscreenme:{i}")));
        }
        None => {
            recipe.insert("source".into(), json!("splitscreenme:unknown"));
            b.note("The script has no Hub.Handler.Id, so the recipe source is splitscreenme:unknown.");
        }
    }

    // match ---------------------------------------------------------------------------------
    let exe = b.p.text("ExecutableName").filter(|s| !s.is_empty());
    let app_id: Option<u64> = b.p.text("SteamID").and_then(|s| s.trim().parse().ok()).filter(|n| *n != 0);
    let mut m = Map::new();
    if let Some(a) = app_id {
        m.insert("steamAppId".into(), json!(a));
    }
    match &exe {
        Some(e) => {
            m.insert("exe".into(), json!(e));
        }
        None => {
            b.note("The handler names no Game.ExecutableName; set match.exe and launch.exe by hand.");
            b.cap = b.cap.min(0.2);
        }
    }
    recipe.insert("match".into(), Value::Object(m));

    // players -------------------------------------------------------------------------------
    let max_players = b
        .p
        .number("MaxPlayers")
        .map(|n| n as u64)
        .or_else(|| b.meta.as_ref().map(|m| m.max_players as u64).filter(|n| *n > 0))
        .unwrap_or(2)
        .max(1);
    // Couchlink seats at most eight players on one PC; the handler's own limit is kept as a note.
    if max_players > 8 {
        b.note(format!("The handler allows up to {max_players} players; Couchlink seats eight on one PC."));
    }
    let max_players = max_players.min(8);
    let min_players = if max_players < 2 { 1 } else { 2 };
    recipe.insert("players".into(), json!([min_players, max_players]));
    recipe.insert("mode".into(), json!("instances"));

    // steam ---------------------------------------------------------------------------------
    let goldberg = b.flag("UseGoldberg");
    let sse = b.flag("NeedsSteamEmulation");
    let steam = if goldberg || sse {
        "goldberg"
    } else if app_id.is_some() {
        "shared"
    } else {
        "none"
    };
    if sse && !goldberg {
        b.note("Handler uses SmartSteamEmu (NeedsSteamEmulation); the recipe uses Goldberg instead.");
        b.penalize("sse", 0.05);
    }
    if goldberg && app_id.is_none() {
        b.note("Handler uses Goldberg but has no Steam app id; set match.steamAppId.");
        b.penalize("goldberg-no-id", 0.15);
    }
    if b.flag("GoldbergExperimentalSteamClient") || b.flag("GoldbergExperimental") {
        b.note("Handler uses the experimental Goldberg steamclient (steamclient_loader); Couchlink's Steam layer uses the regular gbe_fork steam_api swap.");
        b.penalize("goldberg-exp", 0.05);
    }
    if b.flag("SteamlessPatch") || b.p.get("SteamlessPatch").is_some_and(|v| !matches!(v, Val::Bool(false))) {
        let args = b.p.list("SteamlessPatch").join(" ");
        b.note(format!(
            "Handler patches the exe with Steamless ({args}); Couchlink does not patch executables, so a game with Steam DRM wrapping may need an unwrapped exe."
        ));
        b.penalize("steamless", 0.15);
    }
    recipe.insert("steam".into(), json!(steam));
    recipe.insert("present".into(), json!("window"));

    // launch --------------------------------------------------------------------------------
    let binaries = b.p.text("BinariesFolder").map(|s| norm_path(&s)).filter(|s| !s.is_empty());
    let to_launch = b.p.text("ExecutableToLaunch").filter(|s| !s.is_empty());
    let launcher = b.p.text("LauncherExe").filter(|s| !s.is_empty());
    let mut launch_exe = exe.clone();
    if let Some(t) = &to_launch {
        if Some(t) != exe.as_ref() {
            b.note(format!(
                "Handler starts {t} instead of the game exe{}; the game window belongs to the process started from it.",
                exe.as_deref().map(|e| format!(" ({e})")).unwrap_or_default()
            ));
            b.penalize("exe-to-launch", 0.05);
        }
        launch_exe = Some(t.clone());
    }
    if let Some(l) = &launcher {
        b.note(format!(
            "Handler starts the launcher {l}, which then starts {}; the recipe launches the launcher, window placement should follow the game process.",
            exe.as_deref().unwrap_or("the game")
        ));
        b.penalize("launcher", 0.15);
        launch_exe = Some(l.clone());
    }
    let mut launch = Map::new();
    if let Some(e) = &launch_exe {
        let full = match (&binaries, &launcher) {
            (Some(bf), None) => format!("{bf}/{e}"),
            _ => e.clone(),
        };
        launch.insert("exe".into(), json!(full));
    }
    if let Some(bf) = &binaries {
        launch.insert("workdir".into(), json!(bf));
    }

    // arguments: Game.StartArguments, then Context.StartArguments set in Play
    let mut args_val: Option<Val> = b.p.get("StartArguments").cloned();
    let sets: Vec<(String, bool, Val, usize)> = b.p.play_context_sets.clone();
    let mut conditional_args: Vec<Val> = Vec::new();
    for (name, append, v, depth) in &sets {
        if name != "StartArguments" {
            continue;
        }
        if *depth > 1 {
            conditional_args.push(v.clone());
            continue;
        }
        args_val = Some(match (*append, args_val.take()) {
            (true, Some(prev)) => crate::js::add_vals(prev, v.clone()),
            _ => v.clone(),
        });
    }
    let base_empty = match &args_val {
        None => true,
        Some(Val::Str(s)) => s.trim().is_empty(),
        _ => false,
    };
    if !conditional_args.is_empty() {
        // arguments chosen by conditions (keyboard or gamepad instance, user options): take the
        // last branch that resolves fully and list every variant
        let full: Vec<(usize, String)> = conditional_args
            .iter()
            .enumerate()
            .filter_map(|(i, v)| to_template(v).ok().map(|t| (i, t.trim().to_string())))
            .collect();
        let variants: Vec<String> = conditional_args.iter().map(Val::display).collect();
        if base_empty {
            if let Some((i, text)) = full.last() {
                args_val = Some(Val::Str(text.clone()));
                b.note(format!(
                    "Launch arguments depend on conditions in Game.Play ({} variant(s)); the recipe uses variant {} of them. Variants: {}",
                    variants.len(),
                    i + 1,
                    variants.join(" | ")
                ));
                b.penalize("args-cond", 0.05);
            } else {
                b.note(format!(
                    "Launch arguments depend on conditions and script variables in Game.Play and are left empty. Variants: {}",
                    variants.join(" | ")
                ));
                b.penalize("args-cond", 0.1);
            }
        } else {
            b.note(format!(
                "Launch arguments get more parts under conditions in Game.Play: {}",
                variants.join(" | ")
            ));
            b.penalize("args-cond", 0.05);
        }
    }
    let mut args_text = String::new();
    if let Some(v) = &args_val {
        match to_template(v) {
            Ok(s) => args_text = s.trim().to_string(),
            Err(partial) => {
                args_text = partial.trim().to_string();
                b.note(format!(
                    "Launch arguments are built by script logic ({}); the recipe keeps the parts it understands.",
                    v.display()
                ));
                b.penalize("args-dynamic", 0.15);
            }
        }
    }
    launch.insert("args".into(), json!(args_text));
    let mut env = Map::new();
    if let Some(a) = app_id {
        env.insert("SteamAppId".into(), json!(a.to_string()));
    }
    launch.insert("env".into(), Value::Object(env));
    let kill = b.p.list("KillProcessesOnClose");
    if !kill.is_empty() {
        launch.insert("kill_on_exit".into(), json!(kill));
    }
    recipe.insert("launch".into(), Value::Object(launch));

    // files ---------------------------------------------------------------------------------
    let symlink_game = b.p.flag("SymlinkGame").unwrap_or(true);
    let mut copy: Vec<String> = b.p.list("FileSymlinkCopyInstead").iter().map(|s| norm_path(s)).collect();
    copy.dedup();
    let copy_dirs: Vec<String> = b.p.list("DirSymlinkCopyInstead").iter().map(|s| norm_path(s)).collect();
    let mut exclude_files: Vec<String> = Vec::new();
    for f in b.p.list("FileSymlinkExclusions") {
        let f = norm_path(&f);
        if is_input_shim(&f) {
            continue; // Nucleus drops its own xinput/dinput shims here; Couchlink uses a hook instead
        }
        if is_steam_bookkeeping(&f) {
            continue; // the Steam layer writes and swaps these per copy
        }
        exclude_files.push(f);
    }
    let exclude_dirs: Vec<String> = b
        .p
        .list("DirExclusions")
        .iter()
        .map(|s| norm_path(s))
        .filter(|s| !is_steam_bookkeeping(s))
        .collect();
    // saves and configs: user-profile relative and Documents relative folders
    let mut saves: Vec<String> = Vec::new();
    for (key, base) in [
        ("UserProfileSavePath", "$USERPROFILE"),
        ("UserProfileConfigPath", "$USERPROFILE"),
        ("DocumentsSavePath", "$DOCUMENTS"),
        ("DocumentsConfigPath", "$DOCUMENTS"),
    ] {
        if let Some(p) = b.p.text(key).filter(|s| !s.is_empty()) {
            let p = norm_path(&p);
            if p.contains("Goldberg SteamEmu Saves") {
                continue; // per-seat Goldberg saves are the Steam layer's job
            }
            saves.push(format!("{base}/{p}"));
        }
    }
    // keep only the outermost folders
    let all = saves.clone();
    saves.retain(|s| !all.iter().any(|o| o != s && s.starts_with(&format!("{o}/"))));
    saves.sort();
    saves.dedup();
    copy.retain(|c| !c.trim().is_empty());
    let mut files = Map::new();
    files.insert("copy".into(), json!(copy));
    files.insert("link".into(), json!(if symlink_game { "all" } else { "none" }));
    files.insert("saves".into(), json!(saves));
    if !copy_dirs.is_empty() {
        files.insert("copy_dirs".into(), json!(copy_dirs));
    }
    if !exclude_files.is_empty() {
        files.insert("exclude_files".into(), json!(exclude_files));
    }
    if !exclude_dirs.is_empty() {
        files.insert("exclude_dirs".into(), json!(exclude_dirs));
    }
    recipe.insert("files".into(), Value::Object(files));

    // registry (unconditional EditRegKey calls in Play) -----------------------------------
    let calls = b.p.play_calls.clone();
    let mut registry: Vec<Value> = Vec::new();
    for (path, args, depth) in &calls {
        if path != "Context.EditRegKey" {
            continue;
        }
        let hive = match args.first().and_then(Val::as_str) {
            Some("HKEY_CURRENT_USER") | Some("HKCU") => Some("HKCU"),
            Some("HKEY_LOCAL_MACHINE") | Some("HKLM") => Some("HKLM"),
            _ => None,
        };
        let key = args.get(1).and_then(Val::as_str);
        let name = args.get(2).and_then(Val::as_str);
        let value = args.get(3).and_then(|v| match v {
            Val::Num(n) if n.fract() == 0.0 => Some(json!(*n as i64)),
            Val::Num(n) => Some(json!(*n)),
            Val::Str(s) => Some(json!(s)),
            other => to_template(other).ok().map(Value::String),
        });
        match (hive, key, name, value) {
            (Some(h), Some(k), Some(n), Some(v)) if *depth == 1 => {
                registry.push(json!({ "key": format!("{h}\\{k}"), "name": n, "value": v }));
            }
            (Some(h), Some(k), Some(n), _) => {
                let why = if *depth > 1 { "set under a condition" } else { "value computed by script" };
                b.note(format!("Registry value {h}\\{k} \"{n}\" is {why} in Game.Play; add it to registry by hand if needed."));
                b.penalize("registry-cond", 0.05);
            }
            _ => {
                b.note("A Context.EditRegKey call in Game.Play uses computed names; not mapped.");
                b.penalize("registry-cond", 0.05);
            }
        }
    }
    recipe.insert("registry".into(), Value::Array(registry));

    // locks ---------------------------------------------------------------------------------
    let mut locks: Vec<String> = Vec::new();
    for raw in b.p.list("KillMutex") {
        let name = match raw.split_once('|') {
            Some((prefix, rest)) if prefix.starts_with("Type:") => {
                let kind = prefix.trim_start_matches("Type:");
                if !kind.eq_ignore_ascii_case("Mutant") {
                    b.note(format!("Lock \"{rest}\" is a {kind}, not a mutex; the hook renames named objects of every kind."));
                }
                rest.to_string()
            }
            _ => raw,
        };
        if !name.is_empty() && !locks.contains(&name) {
            locks.push(name);
        }
    }
    for h in b.p.list("ProtoInput.RenameHandles") {
        if !h.is_empty() && !locks.contains(&h) {
            locks.push(h);
        }
    }
    if b.flag("PartialMutexSearch") {
        b.note("Handler matches mutex names partially; the lock names in the recipe are exact, so check them.");
        b.penalize("partial-mutex", 0.05);
    }
    for pipe in b.p.list("ProtoInput.RenameNamedPipes") {
        b.note(format!("Named pipe \"{pipe}\" is renamed per instance in the handler; add it to the lock names if the game needs it."));
    }
    recipe.insert("locks".into(), json!(locks));

    // input ---------------------------------------------------------------------------------
    let xinput = b.any_flag(&["Hook.XInputEnabled", "ProtoInput.XinputHook", "ProtoInput.UseOpenXinput", "UseX360ce"])
        || !b.p.list("XInputPlusDll").is_empty();
    let dinput_forced_off = b.flag("Hook.DInputForceDisable");
    let dinput = !dinput_forced_off
        && b.any_flag(&["Hook.DInputEnabled", "ProtoInput.DinputDeviceHook", "ProtoInput.UseDinputRedirection"]);
    let rawinput = b.any_flag(&["ProtoInput.RegisterRawInputHook", "ProtoInput.GetRawInputDataHook"]);
    let keyboard = if b.flag("SupportsMultipleKeyboardsAndMice") {
        "multi"
    } else if b.flag("SupportsKeyboard") {
        "single"
    } else {
        "none"
    };
    recipe.insert(
        "input".into(),
        json!({ "xinput": xinput, "dinput": dinput, "rawinput": rawinput, "keyboard": keyboard }),
    );

    // hook flags ----------------------------------------------------------------------------
    let focus = b.any_flag(&[
        "Hook.ForceFocus",
        "HookFocus",
        "FakeFocus",
        "SendFakeFocusMsg",
        "ProtoInput.FocusHooks",
        "ProtoInput.EnableFocusMessageLoop",
    ]);
    let interval = b
        .p
        .number("FakeFocusInterval")
        .or_else(|| b.p.number("ProtoInput.FocusLoopIntervalMilliseconds"))
        .map(|n| n as u64);
    let cursor = b.any_flag(&[
        "HookSetCursorPos",
        "HookGetCursorPos",
        "HookMouseVisibility",
        "ProtoInput.GetCursorPosHook",
        "ProtoInput.SetCursorPosHook",
        "ProtoInput.CursorVisibilityHook",
    ]);
    let set_window_pos = b.any_flag(&["SetWindowHook", "SetWindowHookStart", "ProtoInput.SetWindowPosHook"]);
    let blocked: Vec<u64> = match b.p.get("ProtoInput.BlockedMessages") {
        Some(Val::Arr(items)) => items.iter().filter_map(Val::as_f64).map(|n| n as u64).collect(),
        _ => Vec::new(),
    };
    let mut hook = Map::new();
    hook.insert("focus".into(), json!(focus));
    if focus {
        if let Some(i) = interval {
            hook.insert("focus_interval_ms".into(), json!(i));
        }
    }
    hook.insert("clip_cursor".into(), json!(b.flag("ProtoInput.ClipCursorHook")));
    hook.insert("cursor".into(), json!(cursor));
    hook.insert("set_window_pos".into(), json!(set_window_pos));
    hook.insert("block_messages".into(), json!(blocked));
    recipe.insert("hook".into(), Value::Object(hook));

    // window --------------------------------------------------------------------------------
    let mut window = Map::new();
    window.insert("borderless".into(), json!(!b.flag("DontRemoveBorders")));
    let pause_starts = b.p.number("PauseBetweenStarts");
    window.insert("settle_ms".into(), json!(pause_starts.map(|s| (s * 1000.0) as u64).unwrap_or(4000)));
    if let Some(g) = b.p.number("PauseBetweenProcessGrab") {
        window.insert("grab_delay_ms".into(), json!((g * 1000.0) as u64));
    }
    if b.p.fields.contains_key("SetTopMostAtEnd") {
        window.insert("topmost".into(), json!(b.flag("SetTopMostAtEnd")));
    }
    if let Some(t) = b.p.text("Hook.ForceFocusWindowName").filter(|s| !s.is_empty()) {
        window.insert("title".into(), json!(t));
        if b.flag("HasDynamicWindowTitle") {
            window.insert("title_partial".into(), json!(true));
        }
    }
    if b.flag("HideTaskbar") {
        window.insert("hide_taskbar".into(), json!(true));
    }
    if b.flag("DontResize") {
        window.insert("keep_size".into(), json!(true));
    }
    if b.flag("DontReposition") {
        window.insert("keep_position".into(), json!(true));
    }
    recipe.insert("window".into(), Value::Object(window));

    // meta ----------------------------------------------------------------------------------
    let mut meta = Map::new();
    if let Some(v) = hub_version {
        meta.insert("hub_version".into(), json!(v));
    }
    if let Some(n) = b.p.hub.get("Maintainer.Name").and_then(Val::as_str) {
        meta.insert("maintainer".into(), json!(n));
    }
    if let Some(m) = &b.meta {
        if let Some(i) = m.igdb_id {
            meta.insert("igdb_id".into(), json!(i));
        }
        meta.insert("hub_title".into(), json!(m.title));
    }
    if !meta.is_empty() {
        recipe.insert("meta".into(), Value::Object(meta));
    }
    if let Some(d) = b.p.text("Description").filter(|s| !s.trim().is_empty()) {
        recipe.insert("instructions".into(), json!(d));
        b.note("The handler has setup instructions (recipe.instructions); they usually name steps done by hand, such as installing a mod or closing Steam.");
    }

    // notes and confidence from everything else ---------------------------------------------
    unsupported_notes(b);
    Value::Object(recipe)
}

fn unsupported_notes(b: &mut Builder) {
    // top-level flags
    for (key, note, pen) in UNSUPPORTED_FLAGS {
        let v = b.p.get(key);
        let on = match v {
            Some(Val::Bool(x)) => *x,
            Some(Val::Str(s)) => !s.is_empty() && s != "false",
            Some(Val::Num(n)) => *n != 0.0,
            Some(Val::Arr(a)) => !a.is_empty(),
            _ => false,
        };
        if on {
            b.note(*note);
            b.penalize(key, *pen);
        }
    }
    for (key, what, pen) in UNSUPPORTED_LISTS {
        let list = b.p.list(key);
        if !list.is_empty() {
            let shown: Vec<String> = list.iter().take(4).cloned().collect();
            b.note(format!("{what}: {}{}", shown.join(", "), if list.len() > 4 { ", ..." } else { "" }));
            b.penalize(key, *pen);
        }
    }

    // fields set only under conditions inside Game.Play
    let conditional: Vec<(String, Vec<Val>)> = b.p.conditional.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (key, vals) in conditional {
        if RUNTIME_KEYS.contains(&key.as_str()) {
            let shown: Vec<String> = vals.iter().take(3).map(Val::display).collect();
            b.note(format!(
                "Game.{key} is set under a condition inside Game.Play (values: {}); the recipe uses the top-level value, if there is one.",
                shown.join(" / ")
            ));
            b.penalize("conditional-fields", 0.05);
        }
    }

    // user options
    if !b.p.options.is_empty() {
        let keys: Vec<String> = b.p.options.iter().map(|o| o.key.clone()).collect();
        b.note(format!(
            "Handler offers {} user option(s) ({}) that only steer Game.Play (graphics presets, fixes); none are applied.",
            keys.len(),
            keys.join(", ")
        ));
        b.penalize("options", 0.05);
    }

    // Game.Play body: group what it does
    let calls = b.p.play_calls.clone();
    let mut ini_keys: BTreeSet<String> = BTreeSet::new();
    let mut ini_edits = 0usize;
    let mut text_edits = 0usize;
    let mut xml_edits = 0usize;
    let mut patches = 0usize;
    let mut written: BTreeSet<String> = BTreeSet::new();
    let mut helpers: BTreeSet<String> = BTreeSet::new();
    let mut processes: BTreeSet<String> = BTreeSet::new();
    let mut fileops: BTreeSet<String> = BTreeSet::new();
    let mut regops: BTreeSet<String> = BTreeSet::new();
    let mut other: BTreeSet<String> = BTreeSet::new();
    let mut proto_calls = 0usize;

    for (path, args, _depth) in &calls {
        match path.as_str() {
            "Nucleus.IniSaveInfo" | "new Nucleus.IniSaveInfo" => {
                ini_edits += 1;
                let sec = args.first().map(Val::display).unwrap_or_default();
                let key = args.get(1).map(Val::display).unwrap_or_default();
                ini_keys.insert(format!("{sec}/{key}"));
            }
            "Context.ModifySaveFile" => {}
            "Context.ReplaceLinesInTextFile" | "Context.ReplaceTextInFile" => text_edits += 1,
            p if p.starts_with("Context.ChangeXml") => xml_edits += 1,
            p if p.starts_with("Context.PatchFile") || p.starts_with("Context.HexEdit") => patches += 1,
            "Context.WriteTextFile" => {
                if let Some(a) = args.first() {
                    let t = tail_literal(a);
                    if !is_goldberg_file(&t) {
                        written.insert(t);
                    }
                }
            }
            "Context.RunAdditionalFiles" => {
                if let Some(Val::Arr(items)) = args.first() {
                    for it in items {
                        let t = tail_literal(it);
                        helpers.insert(t.split_once('|').map(|(_, r)| r.to_string()).unwrap_or(t));
                    }
                }
            }
            "Context.StartProcess" => {
                processes.insert(args.first().map(tail_literal).unwrap_or_default());
            }
            "Context.CopyFolder" | "Context.CopyScriptFolder" | "Context.ExtractZip" => {
                fileops.insert(path.trim_start_matches("Context.").to_string());
            }
            "Context.CreateRegKey" | "Context.DeleteRegKey" => {
                regops.insert(path.trim_start_matches("Context.").to_string());
            }
            "Context.EditRegKey" => {}
            p if p.starts_with("System.IO.") => {
                let op = p.trim_start_matches("System.IO.");
                if !IGNORED_CALLS.contains(&p) {
                    fileops.insert(op.to_string());
                }
            }
            p if p.starts_with("ProtoInput.") || p.starts_with("Context.Hook.") => proto_calls += 1,
            p if IGNORED_CALLS.contains(&p) => {}
            p if p.starts_with("Context.") || p.starts_with("System.") || p.starts_with("Nucleus.") => {
                other.insert(p.to_string());
            }
            _ => {}
        }
    }
    let _ = proto_calls;
    if ini_edits > 0 {
        let shown: Vec<String> = ini_keys.iter().take(8).cloned().collect();
        b.note(format!(
            "Game.Play edits INI settings in {} place(s) ({}{}); the recipe cannot, so set the window mode and resolution in the game by hand or add launch arguments.",
            ini_edits,
            shown.join(", "),
            if ini_keys.len() > 8 { ", ..." } else { "" }
        ));
        b.penalize("ini", 0.1);
    }
    if xml_edits > 0 {
        b.note(format!("Game.Play edits XML settings files ({xml_edits} change(s)); not modelled."));
        b.penalize("xml", 0.08);
    }
    if text_edits > 0 {
        b.note(format!("Game.Play rewrites lines in text/config files ({text_edits} call(s)); not modelled."));
        b.penalize("text", 0.08);
    }
    if patches > 0 {
        b.note(format!("Game.Play patches files in place ({patches} call(s)); not modelled."));
        b.penalize("patch", 0.15);
    }
    if !written.is_empty() {
        let shown: Vec<String> = written.iter().take(6).cloned().collect();
        b.note(format!(
            "Game.Play writes files: {}{}; not modelled (Goldberg's own files are made by Couchlink's Steam layer and are not listed).",
            shown.join(", "),
            if written.len() > 6 { ", ..." } else { "" }
        ));
        b.penalize("write", 0.05);
    }
    if !helpers.is_empty() {
        b.note(format!(
            "Game.Play starts helper programs: {}; a candidate for launch.first, which needs its wait condition set.",
            helpers.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        b.penalize("helpers", 0.15);
    }
    if !processes.is_empty() {
        b.note(format!(
            "Game.Play starts extra processes: {}.",
            processes.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        b.penalize("processes", 0.1);
    }
    if !fileops.is_empty() {
        b.note(format!(
            "Game.Play does file operations ({}); not modelled.",
            fileops.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        b.penalize("fileops", 0.08);
    }
    if !regops.is_empty() {
        b.note(format!(
            "Game.Play creates or deletes registry keys ({}); not modelled.",
            regops.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        b.penalize("regops", 0.05);
    }
    if !other.is_empty() {
        let shown: Vec<String> = other.iter().take(6).cloned().collect();
        b.note(format!("Game.Play also calls: {}.", shown.join(", ")));
        b.penalize("other", (0.02 * other.len() as f32).min(0.1));
    }
    // functions other than Play (stop handlers, hotkey handlers, setup screens)
    let extra: Vec<String> = b
        .p
        .functions
        .iter()
        .filter(|f| {
            f.starts_with("Game.")
                && f.as_str() != "Game.Play"
                && !f.starts_with("Game.ProtoInput.OnInput")
        })
        .cloned()
        .collect();
    if !extra.is_empty() {
        b.note(format!("Handler defines callbacks the recipe cannot run: {}.", extra.join(", ")));
        b.penalize("callbacks", 0.1);
    }
    if b.p.has_play() && b.p.play_calls.is_empty() && b.p.play_context_sets.is_empty() && b.p.conditional.is_empty() {
        // an empty or fully understood Play body: nothing to say
    }
    if !b.p.has_play() {
        b.note("The script has no Game.Play function, so there is nothing per-instance to translate.");
    }
}

/// `slug` cut to at most `max` characters at a word break, so recipe ids stay within 64 characters.
fn short_slug(name: &str, max: usize) -> String {
    let s = slug(name);
    if s.len() <= max {
        return s;
    }
    let cut = &s[..max];
    cut.rfind('-').map(|i| &cut[..i]).unwrap_or(cut).trim_end_matches('-').to_string()
}

/// `$SEAT302` would read as a variable called SEAT302, so a variable followed by a name character
/// becomes `${SEAT}302`. Applies to every string in the recipe.
fn brace_vars(v: &mut Value) {
    match v {
        Value::String(s) if s.contains('$') => {
            let mut out = String::with_capacity(s.len() + 4);
            let b = s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'$' {
                    out.push_str("$$");
                    i += 2;
                    continue;
                }
                if b[i] == b'$' {
                    let start = i + 1;
                    let mut end = start;
                    while end < b.len() && (b[end].is_ascii_uppercase() || b[end].is_ascii_digit() || b[end] == b'_') {
                        end += 1;
                    }
                    let name = &s[start..end];
                    // The longest known name that starts the run; anything after it is literal text.
                    let known = ["SEAT", "PLAYER", "WIDTH", "HEIGHT", "X", "Y", "GAMEDIR", "STEAMID", "USERPROFILE", "DOCUMENTS", "APPDATA", "LOCALAPPDATA"];
                    if let Some(k) = known.iter().filter(|k| name.starts_with(**k) && name.len() > k.len()).max_by_key(|k| k.len()) {
                        out.push_str(&format!("${{{k}}}{}", &name[k.len()..]));
                        i = end;
                        continue;
                    }
                }
                let ch = s[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
            *s = out;
        }
        Value::Array(a) => a.iter_mut().for_each(brace_vars),
        Value::Object(o) => o.values_mut().for_each(brace_vars),
        _ => {}
    }
}
