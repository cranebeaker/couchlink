//! Game recipes: the data that says how to start one copy of a game per player (schema in
//! docs/launch.md). Parsing, validation and template expansion; no I/O.

use crate::recipe_parts::{check_file_template, machine_prefix, one_or_many, Edit, Helper, HelperAt, OptionDef, Patch, PerSeatFile, SeatSel, When, SEAT_VARS};
use crate::screens::Present;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// Schema number this build reads.
pub const SCHEMA: u32 = 1;
/// Template variables the runner always provides.
pub const BUILTIN_VARS: [&str; 12] =
    ["SEAT", "PLAYER", "WIDTH", "HEIGHT", "X", "Y", "GAMEDIR", "STEAMID", "APPDATA", "LOCALAPPDATA", "USERPROFILE", "DOCUMENTS"];
/// Built-in variables that name folders of this PC rather than anything about the seat.
pub const MACHINE_VARS: [&str; 4] = ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "DOCUMENTS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One game process per player.
    #[default]
    Instances,
}

/// How the copies become different Steam players (the "ladder" in docs/launch.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteamMode {
    #[default]
    None,
    Shared,
    Goldberg,
    Accounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMode {
    /// Every seat gets a hardlink tree of the game folder.
    #[default]
    All,
    /// No copies: every seat runs from the same real game folder (for example Nitrox).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub exe: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory; defaults to the folder of the exe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Helpers started once for the whole session (a dedicated server, a proxy), each with its wait
    /// conditions. One object or a list; started in order.
    #[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    pub first: Vec<Helper>,
    /// Helpers started for every seat, with that seat's variables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_seat_helpers: Vec<Helper>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Files {
    /// Paths (relative to the game folder; a folder copies everything under it) that get real copies per seat.
    #[serde(default)]
    pub copy: Vec<String>,
    #[serde(default)]
    pub link: LinkMode,
    /// Paths the game writes saves to, kept per seat: relative to the game folder, or under `$USERPROFILE`,
    /// `$DOCUMENTS`, `$APPDATA` or `$LOCALAPPDATA` (redirected by the hook into the seat's own save folder).
    #[serde(default)]
    pub saves: Vec<String>,
    /// Whole folders that get real copies per seat (the same as listing the folder in `copy`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_dirs: Vec<String>,
    /// Files left out of the copies: a bare name matches at any depth, a path matches only there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_files: Vec<String>,
    /// Folders left out of the copies, matched the same way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_dirs: Vec<String>,
    /// Real files written into each seat's copy, optionally with templated content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_seat: Vec<PerSeatFile>,
    /// Byte patches to files of the copies. Strictly limited, see `Patch`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<Patch>,
}

impl Files {
    /// Everything that gets a real copy: `copy` and `copy_dirs`.
    pub fn real_copies(&self) -> Vec<String> {
        self.copy.iter().chain(&self.copy_dirs).cloned().collect()
    }
}

/// One value to set under HKEY_CURRENT_USER while the session runs. `type` is `string` (REG_SZ) or
/// `dword` (REG_DWORD); when it is left out, a number means dword and text means string. Key, name and
/// text values are templates; a dword may be a template too (`"$WIDTH"`), checked once it is expanded.
/// With `per_seat` the value is set right before each seat's copy starts (for games that read their
/// settings at start), with that seat's variables; otherwise once, before anything starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEdit {
    pub key: String,
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RegistryType>,
    pub value: RegistryValue,
    #[serde(default, skip_serializing_if = "is_false")]
    pub per_seat: bool,
    #[serde(default, skip_serializing_if = "SeatSel::is_all")]
    pub seats: SeatSel,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryType {
    String,
    Dword,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegistryValue {
    Dword(u32),
    Text(String),
}

impl RegistryEdit {
    /// The value to write, checked against `type`. A dword may also be written as a decimal string.
    pub fn typed_value(&self) -> Result<RegistryValue, String> {
        match (self.kind, &self.value) {
            (None, v) => Ok(v.clone()),
            (Some(RegistryType::String), RegistryValue::Text(s)) => Ok(RegistryValue::Text(s.clone())),
            (Some(RegistryType::Dword), RegistryValue::Dword(n)) => Ok(RegistryValue::Dword(*n)),
            (Some(RegistryType::Dword), RegistryValue::Text(s)) => {
                s.trim().parse::<u32>().map(RegistryValue::Dword).map_err(|_| format!("{s:?} is not a number for a dword"))
            }
            (Some(RegistryType::String), RegistryValue::Dword(_)) => Err("type string needs a text value".to_string()),
        }
    }

    /// The key (as `HKCU\...`), name and value to write once the templates are expanded.
    pub fn resolve(&self, expand: &dyn Fn(&str) -> Result<String, String>) -> Result<(String, String, RegistryValue), String> {
        let key = expand(&self.key)?;
        parse_hkcu(&key)?;
        let name = expand(&self.name)?;
        let value = match (&self.value, self.kind) {
            (RegistryValue::Dword(_), Some(RegistryType::String)) => {
                return Err("type string needs a text value".to_string());
            }
            (RegistryValue::Dword(n), _) => RegistryValue::Dword(*n),
            (RegistryValue::Text(t), kind) => {
                let t = expand(t)?;
                match kind {
                    Some(RegistryType::Dword) => t.trim().parse::<u32>().map(RegistryValue::Dword).map_err(|_| format!("{t:?} is not a number for a dword"))?,
                    _ => RegistryValue::Text(t),
                }
            }
        };
        Ok((key, name, value))
    }

    fn has_template(&self) -> bool {
        self.key.contains('$') || self.name.contains('$') || matches!(&self.value, RegistryValue::Text(t) if t.contains('$'))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub xinput: bool,
    pub dinput: bool,
    pub rawinput: bool,
}

impl Default for Input {
    fn default() -> Input {
        Input { xinput: true, dinput: false, rawinput: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowOpts {
    /// Remove the title bar and borders when placing the window.
    pub borderless: bool,
    /// Pause after each copy starts before the next one, so games finish their splash screens.
    pub settle_ms: u64,
    /// How long to wait for a copy's window to appear before giving up.
    pub wait_ms: u64,
    /// For games whose window belongs to another process than the one launched (a launcher that starts
    /// the game): the window's process by exe name. It is looked for among the processes the launch
    /// started, then among all processes that began after the launch. Placement and the job object follow it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

impl Default for WindowOpts {
    fn default() -> WindowOpts {
        WindowOpts { borderless: true, settle_ms: 4000, wait_ms: 60_000, exe: None }
    }
}

fn default_source() -> String {
    "user".to_string()
}

/// A folder holding `file`, searched for under each `near` folder (which may use the machine variables)
/// up to `depth` levels down. It becomes a template variable naming that folder; games.json can set it instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Find {
    pub file: String,
    #[serde(default)]
    pub near: Vec<String>,
    #[serde(default = "default_find_depth")]
    pub depth: u8,
}

/// A file the game folder must hold (relative to it), and the plain hint shown when it is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Require {
    pub file: String,
    pub hint: String,
    /// Text the file must contain, for a file that exists either way but changes when the step is done
    /// (a game assembly a mod patches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
}

fn default_find_depth() -> u8 {
    2
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    pub schema: u32,
    pub id: String,
    pub game: String,
    #[serde(rename = "match", default)]
    pub matching: Match,
    /// Fewest and most players the game takes.
    pub players: [u8; 2],
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub steam: SteamMode,
    #[serde(default)]
    pub present: Present,
    pub launch: Launch,
    #[serde(default)]
    pub files: Files,
    #[serde(default)]
    pub registry: Vec<RegistryEdit>,
    #[serde(default)]
    pub locks: Vec<String>,
    #[serde(default)]
    pub input: Input,
    #[serde(default)]
    pub window: WindowOpts,
    /// Recipe-specific template variables (`$NITROX`): name to value. A value may use the built-in variables.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// Extra Goldberg settings for `steam: goldberg`, read by the Steam layer (couchlink-steam `GoldbergOptions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goldberg: Option<serde_json::Value>,
    /// Helper programs looked up on this PC at start (`$NITROX`): variable name to what to look for.
    #[serde(default)]
    pub find: BTreeMap<String, Find>,
    /// Files the game folder must hold before a start, each with what to tell the player if it does not.
    #[serde(default)]
    pub requires: Vec<Require>,
    /// Choices the player makes before a start; each becomes a template variable named like the option.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, OptionDef>,
    /// Changes to settings files, made for the session and undone (shared files) or kept in the seat's copy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<Edit>,
    /// `builtin`, `imported` or `user`. Set by the loader from where the file was found.
    #[serde(default = "default_source")]
    pub source: String,
}

impl Recipe {
    /// Parse and validate recipe JSON. The error names the bad field.
    pub fn parse(text: &str) -> Result<Recipe, String> {
        let mut de = serde_json::Deserializer::from_str(text);
        let recipe: Recipe = serde_path_to_error::deserialize(&mut de).map_err(|e| format!("{}: {}", e.path(), e.inner()))?;
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("schema is {} but this version reads schema {SCHEMA}", self.schema));
        }
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 64
            && self.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !id_ok {
            return Err(format!("id {:?} must be 1 to 64 characters of a-z, 0-9, - and _", self.id));
        }
        if self.game.trim().is_empty() {
            return Err("game must not be empty".to_string());
        }
        let [min, max] = self.players;
        if min < 1 || max > 4 || min > max {
            return Err(format!("players [{min}, {max}] must satisfy 1 <= min <= max <= 4"));
        }
        if self.launch.exe.trim().is_empty() {
            return Err("launch.exe must not be empty".to_string());
        }
        for (name, value) in &self.vars {
            if !is_var_name(name) {
                return Err(format!("vars.{name}: names are A-Z, 0-9 and _, starting with a letter"));
            }
            if BUILTIN_VARS.contains(&name.as_str()) {
                return Err(format!("vars.{name} is a built-in variable"));
            }
            self.check_template(&format!("vars.{name}"), value, true)?;
        }
        for (name, f) in &self.find {
            if !is_var_name(name) || BUILTIN_VARS.contains(&name.as_str()) || self.vars.contains_key(name) {
                return Err(format!("find.{name}: names are A-Z, 0-9 and _, and must not be built in or also under vars"));
            }
            check_relative(&f.file).map_err(|e| format!("find.{name}.file: {e}"))?;
            if f.depth > 4 {
                return Err(format!("find.{name}.depth must be 4 or less"));
            }
            for near in &f.near {
                self.check_template(&format!("find.{name}.near"), near, true)?;
            }
        }
        for (i, r) in self.requires.iter().enumerate() {
            check_relative(&r.file).map_err(|e| format!("requires[{i}].file: {e}"))?;
            if r.hint.trim().is_empty() {
                return Err(format!("requires[{i}].hint must say what to do"));
            }
        }
        for (name, opt) in &self.options {
            if !is_var_name(name) || BUILTIN_VARS.contains(&name.as_str()) || self.vars.contains_key(name) || self.find.contains_key(name) {
                return Err(format!("options.{name}: names are A-Z, 0-9 and _, and must not be built in or also under vars or find"));
            }
            opt.validate(name)?;
        }
        self.check_template("launch.exe", &self.launch.exe, false)?;
        self.check_template("launch.args", &self.launch.args, false)?;
        if let Some(cwd) = &self.launch.cwd {
            self.check_template("launch.cwd", cwd, false)?;
        }
        for (k, v) in &self.launch.env {
            if k.is_empty() || k.contains('=') || k.contains('\0') {
                return Err(format!("launch.env: bad variable name {k:?}"));
            }
            self.check_template(&format!("launch.env.{k}"), v, false)?;
        }
        for (i, h) in self.launch.first.iter().enumerate() {
            self.check_helper(&format!("launch.first[{i}]"), h, true)?;
        }
        for (i, h) in self.launch.per_seat_helpers.iter().enumerate() {
            self.check_helper(&format!("launch.per_seat_helpers[{i}]"), h, false)?;
        }
        self.validate_files()?;
        for (i, edit) in self.registry.iter().enumerate() {
            let field = format!("registry[{i}]");
            self.check_when(&format!("{field}.when"), &edit.when)?;
            if edit.name.contains('\0') || edit.key.contains('\0') {
                return Err(format!("{field} has a NUL character"));
            }
            for (part, t) in [("key", edit.key.as_str()), ("name", edit.name.as_str())] {
                self.check_seat_template(&format!("{field}.{part}"), t, edit.per_seat)?;
            }
            if let RegistryValue::Text(t) = &edit.value {
                self.check_seat_template(&format!("{field}.value"), t, edit.per_seat)?;
            }
            if !edit.per_seat && !edit.seats.is_all() {
                return Err(format!("{field}.seats needs per_seat"));
            }
            if edit.has_template() {
                // Check the shape with every variable standing in as 1: the key must be an HKCU key and a dword a number.
                let stand_in = self.stand_ins();
                edit.resolve(&|t| expand(t, &stand_in)).map_err(|e| format!("{field}: {e}"))?;
            } else {
                parse_hkcu(&edit.key).map_err(|e| format!("{field}.key: {e}"))?;
                edit.typed_value().map_err(|e| format!("{field}.value: {e}"))?;
            }
        }
        for (i, edit) in self.edits.iter().enumerate() {
            let field = format!("edits[{i}]");
            edit.validate().map_err(|e| format!("{field}: {e}"))?;
            self.check_when(&format!("{field}.when"), &edit.when)?;
            self.check_seat_template(&format!("{field}.file"), &edit.file, edit.per_seat)?;
            for (j, s) in edit.set.iter().enumerate() {
                for t in s.templates() {
                    self.check_seat_template(&format!("{field}.set[{j}]"), t, edit.per_seat)?;
                }
            }
            if !edit.per_seat && !edit.seats.is_all() {
                return Err(format!("{field}.seats needs per_seat"));
            }
        }
        if self.window.settle_ms > 120_000 || self.window.wait_ms == 0 || self.window.wait_ms > 600_000 {
            return Err("window.settle_ms must be up to 120000 and window.wait_ms between 1 and 600000".to_string());
        }
        if let Some(exe) = &self.window.exe {
            let ok = !exe.trim().is_empty() && !exe.contains(['/', '\\', '\0']);
            if !ok {
                return Err(format!("window.exe {exe:?} must be an exe file name such as Game.exe"));
            }
        }
        Ok(())
    }

    fn validate_files(&self) -> Result<(), String> {
        let files = &self.files;
        for (field, list) in [("files.copy", &files.copy), ("files.copy_dirs", &files.copy_dirs), ("files.exclude_files", &files.exclude_files), ("files.exclude_dirs", &files.exclude_dirs)] {
            for p in list {
                check_relative(p).map_err(|e| format!("{field}: {e}"))?;
            }
        }
        for p in &files.saves {
            check_file_template(p).map_err(|e| format!("files.saves: {e}"))?;
            self.check_template("files.saves", p, false)?;
            if p.contains('$') && machine_prefix(p).is_none() {
                return Err(format!("files.saves: {p:?} may only use $USERPROFILE, $DOCUMENTS, $APPDATA or $LOCALAPPDATA at the start"));
            }
        }
        let norm = |s: &String| s.replace('\\', "/").trim_matches('/').to_lowercase();
        let copied: Vec<String> = files.real_copies().iter().map(norm).collect();
        for e in files.exclude_files.iter().chain(&files.exclude_dirs) {
            if copied.contains(&norm(e)) {
                return Err(format!("{e:?} is both copied and excluded"));
            }
        }
        let shapes_copies = !files.per_seat.is_empty() || !files.patches.is_empty() || !files.exclude_files.is_empty() || !files.exclude_dirs.is_empty() || !files.copy_dirs.is_empty();
        if files.link == LinkMode::None && shapes_copies {
            return Err("files.link \"none\" runs every seat from the real game folder, so per_seat, patches, copy_dirs and exclude_* (which shape the copies) do not apply".to_string());
        }
        for (i, f) in files.per_seat.iter().enumerate() {
            let field = format!("files.per_seat[{i}]");
            check_relative(&f.path).map_err(|e| format!("{field}.path: {e}"))?;
            self.check_when(&format!("{field}.when"), &f.when)?;
            if let Some(c) = &f.content {
                self.check_seat_template(&format!("{field}.content"), c, true)?;
            }
        }
        let mut main_exes = vec![self.launch.exe.as_str()];
        main_exes.extend(self.matching.exe.as_deref());
        for (i, p) in files.patches.iter().enumerate() {
            let field = format!("files.patches[{i}]");
            p.validate(&main_exes).map_err(|e| format!("{field}: {e}"))?;
            self.check_when(&format!("{field}.when"), &p.when)?;
        }
        Ok(())
    }

    /// A value for every known variable, to check the shape of a template that needs a real one: the numbers of a
    /// seat are 1, an option has its default, anything else is text.
    fn stand_ins(&self) -> BTreeMap<String, String> {
        let mut m: BTreeMap<String, String> = BUILTIN_VARS.iter().map(|n| (n.to_string(), if ["SEAT", "WIDTH", "HEIGHT", "X", "Y"].contains(n) { "1" } else { "x" }.to_string())).collect();
        m.extend(self.vars.keys().chain(self.find.keys()).map(|n| (n.clone(), "1".to_string())));
        m.extend(self.options.iter().map(|(n, o)| (n.clone(), o.default_value())));
        m
    }

    fn check_helper(&self, field: &str, h: &Helper, shared: bool) -> Result<(), String> {
        // A shared helper runs once from the real game folder: it knows $GAMEDIR but nothing about a seat.
        let check = |f: &str, t: &str| -> Result<(), String> {
            self.check_template(f, t, false)?;
            if shared {
                if let Some(n) = template_vars(t)?.into_iter().find(|n| SEAT_VARS.contains(&n.as_str()) && n != "GAMEDIR") {
                    return Err(format!("{f}: ${n} differs per seat; use launch.per_seat_helpers for a helper that needs it"));
                }
            }
            Ok(())
        };
        if h.exe.trim().is_empty() {
            return Err(format!("{field}.exe must not be empty"));
        }
        check(&format!("{field}.exe"), &h.exe)?;
        check(&format!("{field}.args"), &h.args)?;
        if let Some(c) = &h.cwd {
            check(&format!("{field}.cwd"), c)?;
        }
        for (k, v) in &h.env {
            if k.is_empty() || k.contains('=') || k.contains('\0') {
                return Err(format!("{field}.env: bad variable name {k:?}"));
            }
            check(&format!("{field}.env.{k}"), v)?;
        }
        let stand_in = self.stand_ins();
        for w in &h.wait {
            // A wait may use variables (a port per seat): check the variables, then the shape with stand-in values.
            check(&format!("{field}.wait"), w)?;
            let shape = expand(w, &stand_in).map_err(|e| format!("{field}.wait: {e}"))?;
            Wait::parse(&shape).map_err(|e| format!("{field}.wait: {e}"))?;
        }
        self.check_when(&format!("{field}.when"), &h.when)?;
        if shared {
            if !h.seats.is_all() {
                return Err(format!("{field}.seats: a shared helper starts once; use launch.per_seat_helpers to choose seats"));
            }
        } else if h.at == HelperAt::FirstCopy {
            return Err(format!("{field}.at: first_copy only applies to launch.first"));
        }
        Ok(())
    }

    /// Option names and values used in a `when` must exist.
    fn check_when(&self, field: &str, when: &When) -> Result<(), String> {
        for (name, values) in &when.0 {
            let Some(opt) = self.options.get(name) else {
                return Err(format!("{field}: there is no option {name}"));
            };
            if values.is_empty() {
                return Err(format!("{field}.{name}: needs at least one value"));
            }
            if let Some(v) = values.iter().find(|v| !opt.is_text() && !opt.choices.iter().any(|c| &c.value == *v)) {
                return Err(format!("{field}.{name}: {v:?} is not one of the option's choices"));
            }
        }
        Ok(())
    }

    /// Like `check_template`, and a template that is the same for every seat may not use per-seat variables.
    fn check_seat_template(&self, field: &str, template: &str, seat_ok: bool) -> Result<(), String> {
        self.check_template(field, template, false)?;
        if !seat_ok {
            if let Some(n) = template_vars(template)?.into_iter().find(|n| SEAT_VARS.contains(&n.as_str())) {
                return Err(format!("{field}: ${n} differs per seat, so this needs \"per_seat\": true"));
            }
        }
        Ok(())
    }

    /// The value for every option: what the player chose, else its default. A name or value the recipe does not
    /// offer is an error.
    pub fn resolve_options(&self, given: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
        if let Some(unknown) = given.keys().find(|k| !self.options.contains_key(*k)) {
            let known: Vec<&str> = self.options.keys().map(String::as_str).collect();
            return Err(if known.is_empty() {
                format!("{} has no options (got {unknown})", self.game)
            } else {
                format!("{} has no option {unknown} (options: {})", self.game, known.join(", "))
            });
        }
        self.options
            .iter()
            .map(|(name, def)| def.resolve(given.get(name).map(String::as_str)).map(|v| (name.clone(), v)).map_err(|e| format!("option {name}: {e}")))
            .collect()
    }

    /// Every variable used by a template must be built in or listed in `vars`.
    /// `in_vars` is true while checking a `vars` entry, which may only use built-ins.
    fn check_template(&self, field: &str, template: &str, in_vars: bool) -> Result<(), String> {
        for name in template_vars(template).map_err(|e| format!("{field}: {e}"))? {
            let builtin = BUILTIN_VARS.contains(&name.as_str());
            let option = self.options.contains_key(&name);
            let own = self.vars.contains_key(&name) || self.find.contains_key(&name);
            if !builtin && !option && (in_vars || !own) {
                return Err(format!("{field}: unknown variable ${name} (define it under \"vars\", \"find\" or \"options\")"));
            }
        }
        Ok(())
    }
}

fn is_var_name(s: &str) -> bool {
    let mut it = s.bytes();
    matches!(it.next(), Some(b'A'..=b'Z')) && it.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A relative path that stays inside the game folder.
pub fn check_relative(p: &str) -> Result<(), String> {
    if p.is_empty() || p.contains('\0') {
        return Err(format!("bad path {p:?}"));
    }
    let path = Path::new(p);
    if path.is_absolute() || p.starts_with('/') || p.starts_with('\\') || p.contains(':') {
        return Err(format!("{p:?} must be relative to the game folder"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err(format!("{p:?} must stay inside the game folder"));
    }
    Ok(())
}

/// Split a registry key written `HKCU\...` (or `HKEY_CURRENT_USER\...`) into its sub key. Doubled
/// backslashes (as in the examples, from JSON escaping) collapse to one. Only HKCU is allowed.
pub fn parse_hkcu(key: &str) -> Result<String, String> {
    let mut norm = String::new();
    for ch in key.chars() {
        if ch == '\\' && norm.ends_with('\\') {
            continue;
        }
        norm.push(ch);
    }
    let rest = ["HKCU\\", "HKEY_CURRENT_USER\\"]
        .iter()
        .find_map(|p| norm.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).map(|_| &norm[p.len()..]))
        .ok_or_else(|| format!("{key:?}: only HKCU keys are allowed"))?;
    let rest = rest.trim_matches('\\');
    if rest.is_empty() || rest.contains('\0') {
        return Err(format!("{key:?}: the key needs a sub key"));
    }
    Ok(rest.to_string())
}

/// A wait condition for a helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    /// `port:N`: something listens on TCP or UDP port N on this PC.
    Port(u16),
    /// `delay:MS`: that long, as long as the helper is still running.
    Delay(u64),
    /// `file:PATH`: the file exists (a template).
    File(String),
    /// `window:TEXT`: a visible window on this PC has TEXT in its title (a template).
    Window(String),
    /// `exit`: the helper ends by itself (a setup script).
    Exit,
}

impl Wait {
    pub fn parse(s: &str) -> Result<Wait, String> {
        let s = s.trim();
        if let Some(n) = s.strip_prefix("port:") {
            let port: u16 = n.trim().parse().map_err(|_| format!("bad port in {s:?}"))?;
            if port == 0 {
                return Err(format!("bad port in {s:?}"));
            }
            return Ok(Wait::Port(port));
        }
        if let Some(n) = s.strip_prefix("delay:") {
            let ms: u64 = n.trim().parse().map_err(|_| format!("bad delay in {s:?}"))?;
            if ms > 600_000 {
                return Err("delay is at most 600000 ms".to_string());
            }
            return Ok(Wait::Delay(ms));
        }
        if s == "exit" {
            return Ok(Wait::Exit);
        }
        for (prefix, make) in [("file:", Wait::File as fn(String) -> Wait), ("window:", Wait::Window as fn(String) -> Wait)] {
            if let Some(t) = s.strip_prefix(prefix) {
                let t = t.trim();
                if t.is_empty() {
                    return Err(format!("{s:?} needs something after {prefix}"));
                }
                return Ok(make(t.to_string()));
            }
        }
        Err(format!("{s:?} is not port:N, delay:MS, file:PATH, window:TEXT or exit"))
    }
}

/// Variable names used by a template. `$$` is a literal dollar sign, `${NAME}` ends the name explicitly.
pub fn template_vars(template: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    scan(template, |name| {
        out.push(name.to_string());
        Ok(String::new())
    })?;
    Ok(out)
}

/// Replace `$NAME` and `${NAME}` using `vars`. An unknown name is an error. `$$` gives `$`; a `$` that
/// does not start a name stays as it is.
pub fn expand(template: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    scan(template, |name| vars.get(name).cloned().ok_or_else(|| format!("unknown variable ${name}")))
}

fn scan(template: &str, mut resolve: impl FnMut(&str) -> Result<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
        } else if let Some(inner) = after.strip_prefix('{') {
            let end = inner.find('}').ok_or_else(|| format!("unclosed ${{ in {template:?}"))?;
            let name = &inner[..end];
            if !is_var_name(name) {
                return Err(format!("bad variable name ${{{name}}}"));
            }
            out.push_str(&resolve(name)?);
            rest = &inner[end + 1..];
        } else {
            let len = after.bytes().take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == &b'_').count();
            if len > 0 && after.as_bytes()[0].is_ascii_uppercase() {
                out.push_str(&resolve(&after[..len])?);
                rest = &after[len..];
            } else {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Split an argument string the way a Windows command line reads it: spaces separate, double quotes group
/// (and are dropped), backslashes are ordinary characters. Split before expanding variables so a value
/// with spaces (a player name) stays one argument.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nitrox() -> serde_json::Value {
        json!({
          "schema": 1, "id": "subnautica-nitrox", "game": "Subnautica (Nitrox)",
          "match": { "steamAppId": 264710, "exe": "Subnautica.exe" },
          "players": [2, 4], "mode": "instances", "steam": "shared", "present": "window",
          "launch": {
            "exe": "Subnautica.exe",
            "args": "-screen-fullscreen 0 -popupwindow -screen-width $WIDTH -screen-height $HEIGHT",
            "env": { "SteamAppId": "264710" },
            "first": { "exe": "$NITROX/NitroxServer-Subnautica.exe", "wait": "port:11000" }
          },
          "files": { "copy": [], "link": "all", "saves": [] },
          "registry": [ { "key": "HKCU\\\\SOFTWARE\\\\Nitrox", "name": "IsMultipleGameInstancesAllowed", "type": "string", "value": "True" } ],
          "locks": [],
          "input": { "xinput": true, "dinput": false, "rawinput": false },
          "window": { "borderless": true, "settle_ms": 4000 },
          "vars": { "NITROX": "C:/Nitrox" },
          "source": "builtin"
        })
    }

    #[test]
    fn the_example_recipe_parses() {
        let r = Recipe::parse(&nitrox().to_string()).unwrap();
        assert_eq!(r.id, "subnautica-nitrox");
        assert_eq!(r.matching.steam_app_id, Some(264710));
        assert_eq!(r.players, [2, 4]);
        assert_eq!(r.steam, SteamMode::Shared);
        assert_eq!(r.files.link, LinkMode::All);
        assert_eq!(r.registry[0].kind, Some(RegistryType::String));
        assert_eq!(r.registry[0].typed_value(), Ok(RegistryValue::Text("True".into())));
        assert_eq!(r.window.settle_ms, 4000);
        assert_eq!(r.window.wait_ms, 60_000, "unset fields take their defaults");
        assert_eq!(parse_hkcu(&r.registry[0].key).unwrap(), "SOFTWARE\\Nitrox");
        // It round-trips through JSON.
        let again = Recipe::parse(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(again, r);
    }

    #[test]
    fn registry_types_are_explicit_or_inferred() {
        let edit = |t: &str, v: serde_json::Value| -> RegistryEdit {
            let mut e = json!({"key": "HKCU\\X", "name": "n", "value": v});
            if !t.is_empty() {
                e["type"] = json!(t);
            }
            serde_json::from_value(e).unwrap()
        };
        assert_eq!(edit("dword", json!(1)).typed_value(), Ok(RegistryValue::Dword(1)));
        assert_eq!(edit("dword", json!("7")).typed_value(), Ok(RegistryValue::Dword(7)));
        assert_eq!(edit("string", json!("True")).typed_value(), Ok(RegistryValue::Text("True".into())));
        assert_eq!(edit("", json!(1)).typed_value(), Ok(RegistryValue::Dword(1)));
        assert_eq!(edit("", json!("x")).typed_value(), Ok(RegistryValue::Text("x".into())));
        assert!(edit("string", json!(1)).typed_value().is_err());
    }

    #[test]
    fn minimal_recipe_takes_defaults() {
        let r = Recipe::parse(r#"{"schema":1,"id":"a","game":"A","players":[1,2],"launch":{"exe":"a.exe"}}"#).unwrap();
        assert_eq!(r.steam, SteamMode::None);
        assert_eq!(r.present, Present::Window);
        assert!(r.input.xinput && !r.input.dinput);
        assert!(r.window.borderless);
        assert_eq!(r.source, "user");
    }

    fn broken(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v = nitrox();
        edit(&mut v);
        Recipe::parse(&v.to_string()).unwrap_err()
    }

    #[test]
    fn bad_recipes_name_the_field() {
        assert!(broken(|v| v["schema"] = json!(2)).contains("schema"));
        assert!(broken(|v| v["id"] = json!("Bad Id")).contains("id"));
        assert!(broken(|v| v["id"] = json!("../x")).contains("id"));
        assert!(broken(|v| v["players"] = json!([3, 2])).contains("players"));
        assert!(broken(|v| v["players"] = json!([1, 5])).contains("players"));
        assert!(broken(|v| v["steam"] = json!("maybe")).contains("steam"));
        assert!(broken(|v| v["launch"]["exe"] = json!("")).contains("launch.exe"));
        assert!(broken(|v| v["launch"]["args"] = json!("$NOPE")).contains("$NOPE"));
        assert!(broken(|v| v["launch"]["first"]["wait"] = json!("soon")).contains("wait"));
        assert!(broken(|v| v["files"]["copy"] = json!(["../outside.ini"])).contains("files.copy"));
        assert!(broken(|v| v["files"]["copy"] = json!(["C:\\x.ini"])).contains("files.copy"));
        assert!(broken(|v| v["files"]["link"] = json!("some")).contains("files.link"));
        assert!(broken(|v| v["registry"][0]["key"] = json!("HKLM\\SOFTWARE\\X")).contains("HKCU"));
        assert!(broken(|v| v["registry"][0]["value"] = json!(true)).contains("registry"));
        assert!(broken(|v| v["registry"][0]["value"] = json!(1)).contains("type string"));
        assert!(broken(|v| v["registry"][0]["type"] = json!("binary")).contains("registry"));
        assert!(broken(|v| {
            v["registry"][0]["type"] = json!("dword");
            v["registry"][0]["value"] = json!("True");
        })
        .contains("not a number"));
        assert!(broken(|v| v["vars"]["seat"] = json!("x")).contains("vars"));
        assert!(broken(|v| v["vars"]["SEAT"] = json!("x")).contains("built-in"));
        assert!(broken(|v| v["vars"]["BAD"] = json!("$OTHER")).contains("$OTHER"));
        assert!(Recipe::parse("not json").is_err());
        assert!(Recipe::parse("{}").is_err());
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn templates_expand() {
        let v = vars(&[("SEAT", "1"), ("WIDTH", "960"), ("HEIGHT", "1080"), ("PLAYER", "Player 2"), ("GAMEDIR", "C:/c/1")]);
        assert_eq!(expand("-w $WIDTH -h $HEIGHT", &v).unwrap(), "-w 960 -h 1080");
        assert_eq!(expand("$WIDTHx$HEIGHT", &v).unwrap(), "960x1080", "lower case ends a name");
        assert_eq!(expand("${SEAT}0", &v).unwrap(), "10");
        assert_eq!(expand("$GAMEDIR/save$SEAT.ini", &v).unwrap(), "C:/c/1/save1.ini");
        assert_eq!(expand("cost $$5 and $5 and $", &v).unwrap(), "cost $5 and $5 and $");
        assert!(expand("$MISSING", &v).unwrap_err().contains("MISSING"));
        assert!(expand("${SEAT", &v).is_err());
        assert!(expand("${seat}", &v).is_err());
        assert_eq!(template_vars("$SEAT $$X ${HEIGHT} $5").unwrap(), vec!["SEAT", "HEIGHT"]);
    }

    #[test]
    fn args_split_like_a_command_line() {
        assert_eq!(split_args("-a  -b \"two words\" c\\d"), vec!["-a", "-b", "two words", "c\\d"]);
        assert_eq!(split_args("  "), Vec::<String>::new());
        assert_eq!(split_args("\"\" x"), vec!["", "x"]);
        assert_eq!(split_args("--name \"$PLAYER\""), vec!["--name", "$PLAYER"]);
    }

    #[test]
    fn hkcu_keys_and_waits() {
        assert_eq!(parse_hkcu("HKCU\\Software\\X").unwrap(), "Software\\X");
        assert_eq!(parse_hkcu("hkey_current_user\\\\Software\\\\X").unwrap(), "Software\\X");
        assert!(parse_hkcu("HKLM\\Software").is_err());
        assert!(parse_hkcu("HKCU\\").is_err());
        assert!(parse_hkcu("Software\\X").is_err());
        assert_eq!(Wait::parse("port:11000"), Ok(Wait::Port(11000)));
        assert_eq!(Wait::parse("delay:500"), Ok(Wait::Delay(500)));
        assert!(Wait::parse("port:0").is_err());
        assert!(Wait::parse("port:70000").is_err());
        assert!(Wait::parse("delay:x").is_err());
    }

    #[test]
    fn relative_paths() {
        assert!(check_relative("bin/game.ini").is_ok());
        assert!(check_relative("a\\b").is_ok());
        assert!(check_relative("..\\x").is_err());
        assert!(check_relative("a/../../x").is_err());
        assert!(check_relative("/etc").is_err());
        assert!(check_relative("").is_err());
    }

    // ---- schema for SplitScreen-style setups ------------------------------------------------------

    fn rich() -> serde_json::Value {
        json!({
          "schema": 1, "id": "rich", "game": "Rich", "players": [2, 4],
          "launch": {
            "exe": "Game.exe", "args": "-w $WIDTH -h $HEIGHT $FPSARG --mode $MODE",
            "first": [
              {"exe": "$GAMEDIR/server.exe", "wait": ["port:7777", "delay:200"]},
              {"exe": "tool.exe", "wait": "file:$GAMEDIR/ready.txt", "at": "first_copy"}
            ],
            "per_seat_helpers": [{"exe": "proxy.exe", "args": "--seat $SEAT", "wait": "window:Proxy", "at": "placed", "seats": "1+", "when": {"MODE": "lan"}}]
          },
          "options": {
            "MODE": {"label": "Mode", "choices": ["lan", {"value": "online", "label": "Online"}], "default": "lan"},
            "FPSARG": {"label": "Unlock fps", "choices": [{"value": "", "label": "No"}, {"value": "-fps 0", "label": "Yes"}]},
            "FOV": {"label": "Field of view", "default": "90"}
          },
          "files": {
            "copy": ["config"], "copy_dirs": ["mods/cursor"], "exclude_files": ["winmm.dll", "bin/extra.dll"], "exclude_dirs": ["Launcher"],
            "saves": ["$USERPROFILE/AppData/Local/Rich", "saves"],
            "per_seat": [{"path": "user.ini", "content": "fov=$FOV\nseat=$SEAT\n", "when": {"MODE": "lan"}}, {"path": "config/base.ini"}],
            "patches": [{"file": "data/UnityPlayer.dll", "purpose": "other-safe", "find": {"utf16": "Windows.Gaming.Input.Gamepad"}, "replace": {"zeros": true}, "sha256": "a".repeat(64)}]
          },
          "edits": [
            {"file": "config/game.ini", "format": "ini", "per_seat": true, "set": [{"section": "Display", "key": "Width", "value": "$WIDTH"}]},
            {"file": "$USERPROFILE/AppData/Local/Rich/settings.json", "format": "json", "set": [{"pointer": "/fov", "value": "$FOV", "as": "number"}]},
            {"file": "Config.xml", "format": "xml", "seats": "0", "per_seat": true, "set": [{"path": "//System", "attr": "Width", "value": "$WIDTH"}]},
            {"file": "readme.txt", "format": "text", "optional": true, "when": {"MODE": ["lan", "online"]}, "set": [{"find": "a", "replace": "$MODE"}]}
          ],
          "registry": [
            {"key": "HKCU\\Software\\Rich", "name": "Width", "type": "dword", "value": "$WIDTH", "per_seat": true},
            {"key": "HKCU\\Software\\Rich$SEAT", "name": "Name", "value": "$PLAYER", "per_seat": true, "seats": "0,1"},
            {"key": "HKCU\\Software\\Rich", "name": "Mode", "value": "$MODE", "when": {"MODE": "online"}}
          ],
          "window": {"exe": "RichGame.exe"}
        })
    }

    #[test]
    fn the_rich_recipe_parses_and_round_trips() {
        let r = Recipe::parse(&rich().to_string()).unwrap();
        assert_eq!(r.launch.first.len(), 2);
        assert_eq!(r.launch.first[0].wait, vec!["port:7777", "delay:200"]);
        assert_eq!(r.launch.per_seat_helpers[0].seats, SeatSel::From(1));
        assert_eq!(r.options["FPSARG"].default_value(), "");
        assert_eq!(r.files.real_copies(), vec!["config", "mods/cursor"]);
        assert_eq!(r.window.exe.as_deref(), Some("RichGame.exe"));
        assert_eq!(Recipe::parse(&serde_json::to_string(&r).unwrap()).unwrap(), r);
        // `first` may still be one object.
        let mut v = nitrox();
        v["launch"]["first"] = json!({"exe": "$NITROX/s.exe", "wait": "port:1"});
        assert_eq!(Recipe::parse(&v.to_string()).unwrap().launch.first.len(), 1);
    }

    fn rich_broken(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v = rich();
        edit(&mut v);
        Recipe::parse(&v.to_string()).unwrap_err()
    }

    #[test]
    fn bad_options_name_the_field() {
        assert!(rich_broken(|v| v["options"]["mode"] = json!({"label": "x"})).contains("options.mode"));
        assert!(rich_broken(|v| v["options"]["SEAT"] = json!({"label": "x"})).contains("options.SEAT"));
        assert!(rich_broken(|v| v["options"]["MODE"]["default"] = json!("nope")).contains("default"));
        assert!(rich_broken(|v| v["options"]["MODE"]["label"] = json!(" ")).contains("label"));
        assert!(rich_broken(|v| v["vars"] = json!({"MODE": "x"})).contains("options.MODE"));
        assert!(rich_broken(|v| v["launch"]["args"] = json!("$UNDECLARED")).contains("$UNDECLARED"));
        // An option can be used by `vars` and in `when`.
        let mut v = rich();
        v["vars"] = json!({"WORLD": "w-$MODE"});
        assert!(Recipe::parse(&v.to_string()).is_ok());
        assert!(rich_broken(|v| v["edits"][3]["when"] = json!({"NOPE": "x"})).contains("no option NOPE"));
        assert!(rich_broken(|v| v["edits"][3]["when"] = json!({"MODE": "other"})).contains("not one of"));
        assert!(rich_broken(|v| v["registry"][2]["when"] = json!({"MODE": []})).contains("at least one"));
    }

    #[test]
    fn options_resolve_against_what_the_player_gave() {
        let r = Recipe::parse(&rich().to_string()).unwrap();
        let given = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let all = r.resolve_options(&given(&[])).unwrap();
        assert_eq!((all["MODE"].as_str(), all["FPSARG"].as_str(), all["FOV"].as_str()), ("lan", "", "90"));
        let all = r.resolve_options(&given(&[("MODE", "online"), ("FPSARG", "-fps 0"), ("FOV", "110")])).unwrap();
        assert_eq!((all["MODE"].as_str(), all["FPSARG"].as_str(), all["FOV"].as_str()), ("online", "-fps 0", "110"));
        assert!(r.resolve_options(&given(&[("MODE", "bogus")])).unwrap_err().contains("option MODE"));
        assert!(r.resolve_options(&given(&[("NOPE", "x")])).unwrap_err().contains("no option NOPE"));
        assert!(r.resolve_options(&given(&[("FOV", "9;rm")])).is_err());
        let plain = Recipe::parse(&nitrox().to_string()).unwrap();
        assert!(plain.resolve_options(&given(&[("A", "b")])).unwrap_err().contains("no options"));
        assert!(plain.resolve_options(&given(&[])).unwrap().is_empty());
    }

    #[test]
    fn edits_are_checked_against_the_recipe() {
        assert!(rich_broken(|v| v["edits"][0]["set"][0]["value"] = json!("$NOPE")).contains("$NOPE"));
        assert!(rich_broken(|v| v["edits"][0]["set"] = json!([])).contains("edits[0]"));
        assert!(rich_broken(|v| v["edits"][0]["file"] = json!("../x.ini")).contains("edits[0]"));
        assert!(rich_broken(|v| v["edits"][0]["format"] = json!("yaml")).contains("format"));
        // Without per_seat an edit is the same for everyone, so it cannot use the window size of one seat.
        assert!(rich_broken(|v| v["edits"][0]["per_seat"] = json!(false)).contains("per_seat"));
        assert!(rich_broken(|v| v["edits"][3]["seats"] = json!("1+")).contains("seats needs per_seat"));
        assert!(rich_broken(|v| v["edits"][1]["set"][0]["as"] = json!("hex")).contains("as"));
        let mut v = rich();
        v["edits"][0]["per_seat"] = json!(false);
        v["edits"][0]["set"][0]["value"] = json!("$FOV");
        assert!(Recipe::parse(&v.to_string()).is_ok(), "options are the same for every seat");
    }

    #[test]
    fn files_sections_are_checked() {
        assert!(rich_broken(|v| v["files"]["exclude_files"] = json!(["config"])).contains("both copied and excluded"));
        assert!(rich_broken(|v| v["files"]["copy_dirs"] = json!(["../x"])).contains("files.copy_dirs"));
        assert!(rich_broken(|v| v["files"]["exclude_dirs"] = json!(["C:\\x"])).contains("files.exclude_dirs"));
        assert!(rich_broken(|v| v["files"]["saves"] = json!(["$NOPE/x"])).contains("files.saves"));
        assert!(rich_broken(|v| v["files"]["saves"] = json!(["x/$USERPROFILE"])).contains("files.saves"));
        assert!(rich_broken(|v| v["files"]["saves"] = json!(["$USERPROFILE/../x"])).contains("files.saves"));
        assert!(rich_broken(|v| v["files"]["link"] = json!("none")).contains("link"));
        assert!(rich_broken(|v| v["files"]["per_seat"][0]["path"] = json!("../x")).contains("files.per_seat[0].path"));
        assert!(rich_broken(|v| v["files"]["per_seat"][0]["content"] = json!("$NOPE")).contains("$NOPE"));
        assert!(rich_broken(|v| v["files"]["per_seat"][0]["when"] = json!({"X": "1"})).contains("files.per_seat[0].when"));
    }

    #[test]
    fn patches_cannot_touch_drm_or_the_exe() {
        let patch = |file: &str, purpose: &str| {
            let file = file.to_string();
            let purpose = purpose.to_string();
            rich_broken(move |v| {
                v["files"]["patches"][0]["file"] = json!(file);
                v["files"]["patches"][0]["purpose"] = json!(purpose);
            })
        };
        assert!(patch("Game.exe", "lan").contains("executable"));
        assert!(patch("steam_api64.dll", "lan").contains("Steam"));
        assert!(patch("Plugins/EOSSDK-Win64-Shipping.dll", "port").contains("Epic"));
        assert!(patch("EasyAntiCheat/EasyAntiCheat_EOS.dll", "port").contains("anti-cheat"));
        assert!(patch("data/x.dll", "remove-drm").contains("remove-drm"));
        let mut v = rich();
        v["match"] = json!({"exe": "Launcher.exe"});
        v["files"]["patches"][0]["file"] = json!("Launcher.exe");
        assert!(Recipe::parse(&v.to_string()).unwrap_err().contains("executable"), "the match exe counts as the main exe");
        let mut v = rich();
        v["files"]["patches"][0]["note"] = json!("x");
        assert!(Recipe::parse(&v.to_string()).is_err(), "unknown patch fields are refused");
    }

    #[test]
    fn helpers_are_checked() {
        assert!(rich_broken(|v| v["launch"]["first"][0]["wait"] = json!(["soon"])).contains("wait"));
        assert!(rich_broken(|v| v["launch"]["first"][0]["wait"] = json!("file:")).contains("file:"));
        assert!(rich_broken(|v| v["launch"]["first"][0]["args"] = json!("--seat $SEAT")).contains("per_seat_helpers"));
        assert!(rich_broken(|v| v["launch"]["first"][0]["seats"] = json!("0")).contains("seats"));
        assert!(rich_broken(|v| v["launch"]["per_seat_helpers"][0]["at"] = json!("first_copy")).contains("first_copy"));
        assert!(rich_broken(|v| v["launch"]["first"][0]["exe"] = json!(" ")).contains("exe"));
        assert!(rich_broken(|v| v["launch"]["first"][1]["when"] = json!({"NOPE": "1"})).contains("no option"));
        let mut v = rich();
        v["launch"]["first"][0]["wait"] = json!(["exit"]);
        assert!(Recipe::parse(&v.to_string()).is_ok());
        assert_eq!(Wait::parse("file:C:/x/ready.txt"), Ok(Wait::File("C:/x/ready.txt".into())));
        assert_eq!(Wait::parse("window:My Game"), Ok(Wait::Window("My Game".into())));
        assert_eq!(Wait::parse("exit"), Ok(Wait::Exit));
        assert!(Wait::parse("window:").is_err());
    }

    #[test]
    fn registry_templates_are_checked_after_expansion() {
        assert!(rich_broken(|v| v["registry"][0]["value"] = json!("$PLAYER")).contains("not a number"));
        assert!(rich_broken(|v| v["registry"][0]["per_seat"] = json!(false)).contains("per_seat"));
        assert!(rich_broken(|v| v["registry"][1]["key"] = json!("HKLM\\Software\\$SEAT")).contains("HKCU"));
        assert!(rich_broken(|v| v["registry"][1]["seats"] = json!("5")).contains("registry"));
        assert!(rich_broken(|v| v["registry"][2]["seats"] = json!("1")).contains("seats needs per_seat"));
        assert!(rich_broken(|v| v["registry"][0]["value"] = json!("$NOPE")).contains("$NOPE"));
        let r = Recipe::parse(&rich().to_string()).unwrap();
        let vars: BTreeMap<String, String> = [("WIDTH", "960"), ("SEAT", "1"), ("PLAYER", "Ada"), ("MODE", "lan")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let ex = |t: &str| expand(t, &vars);
        assert_eq!(r.registry[0].resolve(&ex).unwrap(), ("HKCU\\Software\\Rich".to_string(), "Width".to_string(), RegistryValue::Dword(960)));
        assert_eq!(r.registry[1].resolve(&ex).unwrap().0, "HKCU\\Software\\Rich1");
        assert_eq!(r.registry[1].resolve(&ex).unwrap().2, RegistryValue::Text("Ada".into()));
        let bad: BTreeMap<String, String> = [("WIDTH".to_string(), "wide".to_string())].into();
        assert!(r.registry[0].resolve(&|t| expand(t, &bad)).unwrap_err().contains("not a number"));
    }

    #[test]
    fn window_exe_is_a_file_name() {
        assert!(rich_broken(|v| v["window"]["exe"] = json!("bin/Game.exe")).contains("window.exe"));
        assert!(rich_broken(|v| v["window"]["exe"] = json!("")).contains("window.exe"));
    }
}
