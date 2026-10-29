//! Parts of the recipe schema for SplitScreen-style setups: options the player picks, edits to settings
//! files, per-seat files, strictly limited byte patches, helper programs and seat/option selectors.
//! Parsing and validation only; applying them is the session runner's job (docs/launch.md, "Recipe").

use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// Variables whose value differs from seat to seat. A shared (not `per_seat`) edit may not use them.
pub const SEAT_VARS: [&str; 8] = ["SEAT", "PLAYER", "WIDTH", "HEIGHT", "X", "Y", "GAMEDIR", "STEAMID"];

/// Accept either one value or a list of values.
pub fn one_or_many<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        Many(Vec<T>),
        One(T),
    }
    Ok(match OneOrMany::<T>::deserialize(d)? {
        OneOrMany::Many(v) => v,
        OneOrMany::One(x) => vec![x],
    })
}

// ---- seat selector ----------------------------------------------------------------------------

/// Which seats a thing applies to: `"all"` (the default), `"1+"` (seat 1 and up), or a list such as `"0,2"`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SeatSel {
    #[default]
    All,
    From(u8),
    List(Vec<u8>),
}

impl SeatSel {
    pub fn parse(s: &str) -> Result<SeatSel, String> {
        let s = s.trim();
        let bad = || format!("{s:?} is not a seat selector (use \"all\", \"1+\" or \"0,2\")");
        let seat = |t: &str| t.trim().parse::<u8>().ok().filter(|n| *n < 4).ok_or_else(bad);
        if s.eq_ignore_ascii_case("all") {
            return Ok(SeatSel::All);
        }
        if let Some(n) = s.strip_suffix('+') {
            return Ok(SeatSel::From(seat(n)?));
        }
        let mut list = s.split(',').map(seat).collect::<Result<Vec<_>, _>>()?;
        list.sort_unstable();
        list.dedup();
        Ok(SeatSel::List(list))
    }

    pub fn matches(&self, seat: usize) -> bool {
        match self {
            SeatSel::All => true,
            SeatSel::From(n) => seat >= *n as usize,
            SeatSel::List(l) => l.iter().any(|n| *n as usize == seat),
        }
    }

    pub fn is_all(&self) -> bool {
        *self == SeatSel::All
    }
}

impl fmt::Display for SeatSel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeatSel::All => write!(f, "all"),
            SeatSel::From(n) => write!(f, "{n}+"),
            SeatSel::List(l) => write!(f, "{}", l.iter().map(u8::to_string).collect::<Vec<_>>().join(",")),
        }
    }
}

impl Serialize for SeatSel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SeatSel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<SeatSel, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Number(u8),
        }
        match Raw::deserialize(d)? {
            Raw::Text(s) => SeatSel::parse(&s),
            Raw::Number(n) => SeatSel::parse(&n.to_string()),
        }
        .map_err(serde::de::Error::custom)
    }
}

// ---- when (option conditions) -------------------------------------------------------------------

/// Applies only when every named option has one of the listed values: `{"FPS": "Yes", "MODE": ["a", "b"]}`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct When(pub BTreeMap<String, Vec<String>>);

impl When {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// True when `options` (name to chosen value) meets every condition.
    pub fn holds(&self, options: &BTreeMap<String, String>) -> bool {
        self.0.iter().all(|(name, values)| options.get(name).is_some_and(|v| values.contains(v)))
    }
}

impl Serialize for When {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, if v.len() == 1 { serde_json::json!(v[0]) } else { serde_json::json!(v) })))
    }
}

impl<'de> Deserialize<'de> for When {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<When, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Strs {
            One(String),
            Many(Vec<String>),
        }
        let raw: BTreeMap<String, Strs> = BTreeMap::deserialize(d)?;
        Ok(When(
            raw.into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        match v {
                            Strs::One(s) => vec![s],
                            Strs::Many(l) => l,
                        },
                    )
                })
                .collect(),
        ))
    }
}

// ---- options ------------------------------------------------------------------------------------

/// One value an option can take. Written as `"Yes"` (value and label alike) or `{"value": "", "label": "No"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Choice, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Plain(String),
            Full { value: String, label: Option<String> },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Plain(s) => Choice { label: s.clone(), value: s },
            Raw::Full { value, label } => Choice { label: label.unwrap_or_else(|| value.clone()), value },
        })
    }
}

/// A choice the player makes before the session starts; the chosen value becomes the template variable
/// named after the option. With no `choices` the option takes short free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionDef {
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    /// The value used when the player does not pick one; the first choice when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Longest free-text option value.
pub const MAX_TEXT_OPTION: usize = 64;

/// Free text must stay harmless inside file edits, registry values and arguments.
pub fn check_text_option(value: &str) -> Result<(), String> {
    let ok = value.len() <= MAX_TEXT_OPTION && value.chars().all(|c| c.is_alphanumeric() || " ._:,/\\+=@-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!("{value:?} is not allowed: up to {MAX_TEXT_OPTION} letters, digits, spaces and . _ : , / \\ + = @ -"))
    }
}

impl OptionDef {
    pub fn is_text(&self) -> bool {
        self.choices.is_empty()
    }

    pub fn default_value(&self) -> String {
        self.default.clone().or_else(|| self.choices.first().map(|c| c.value.clone())).unwrap_or_default()
    }

    /// The value to use: `given` after checking it, or the default.
    pub fn resolve(&self, given: Option<&str>) -> Result<String, String> {
        let Some(v) = given else { return Ok(self.default_value()) };
        if self.is_text() {
            check_text_option(v)?;
        } else if !self.choices.iter().any(|c| c.value == v) {
            let all: Vec<String> = self.choices.iter().map(|c| format!("{:?}", c.value)).collect();
            return Err(format!("{v:?} is not one of {}", all.join(", ")));
        }
        Ok(v.to_string())
    }

    pub fn validate(&self, name: &str) -> Result<(), String> {
        if self.label.trim().is_empty() {
            return Err(format!("options.{name}.label must not be empty"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.choices {
            if !seen.insert(&c.value) {
                return Err(format!("options.{name}.choices: {:?} is listed twice", c.value));
            }
            if c.value.contains('\0') || c.value.contains('\n') || c.value.contains('\r') {
                return Err(format!("options.{name}.choices: bad value {:?}", c.value));
            }
        }
        match (&self.default, self.is_text()) {
            (Some(d), true) => check_text_option(d).map_err(|e| format!("options.{name}.default: {e}"))?,
            (Some(d), false) if !self.choices.iter().any(|c| &c.value == d) => {
                return Err(format!("options.{name}.default {d:?} is not one of the choices"));
            }
            _ => {}
        }
        Ok(())
    }
}

// ---- edits --------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditFormat {
    Ini,
    Xml,
    Json,
    Text,
}

/// How a JSON value written from a template is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonType {
    String,
    Number,
    Bool,
}

/// One change inside an edited file. Which fields are used depends on the file's `format`:
/// ini `section`+`key`+`value`; xml `path`+(`attr`)+`value`; json `pointer`+`value`(+`as`);
/// text `find`+`replace`, or `starts_with` with `set` (a whole line) or `remove`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub json_type: Option<JsonType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove: bool,
    /// text `starts_with`+`set`: add the line at the end when no line starts that way.
    #[serde(default, skip_serializing_if = "is_false")]
    pub add: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Setting {
    fn present(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (name, on) in [
            ("section", self.section.is_some()),
            ("key", self.key.is_some()),
            ("path", self.path.is_some()),
            ("attr", self.attr.is_some()),
            ("pointer", self.pointer.is_some()),
            ("value", self.value.is_some()),
            ("as", self.json_type.is_some()),
            ("find", self.find.is_some()),
            ("replace", self.replace.is_some()),
            ("starts_with", self.starts_with.is_some()),
            ("set", self.set.is_some()),
            ("remove", self.remove),
            ("add", self.add),
        ] {
            if on {
                v.push(name);
            }
        }
        v
    }

    /// Every template string the setting holds (for variable checks), with a flag for JSON values.
    pub fn templates(&self) -> Vec<&str> {
        let mut v: Vec<&str> = [&self.section, &self.key, &self.path, &self.attr, &self.pointer, &self.find, &self.replace, &self.starts_with, &self.set]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if let Some(val) = &self.value {
            collect_strings(val, &mut v);
        }
        v
    }

    pub fn validate(&self, format: EditFormat) -> Result<(), String> {
        let (needs, allowed): (&[&str], &[&str]) = match format {
            EditFormat::Ini => (&["key", "value"], &["section", "key", "value"]),
            EditFormat::Xml => (&["path", "value"], &["path", "attr", "value"]),
            EditFormat::Json => (&["pointer", "value"], &["pointer", "value", "as"]),
            EditFormat::Text => (&[], &["find", "replace", "starts_with", "set", "remove", "add"]),
        };
        let present = self.present();
        if let Some(extra) = present.iter().find(|p| !allowed.contains(p)) {
            return Err(format!("\"{extra}\" does not belong in a {format:?} edit").to_lowercase());
        }
        if let Some(missing) = needs.iter().find(|n| !present.contains(n)) {
            return Err(format!("a {format:?} edit needs \"{missing}\"").replace("Ini", "ini").replace("Xml", "xml").replace("Json", "json"));
        }
        match format {
            EditFormat::Ini => {
                if self.key.as_deref().unwrap_or("").is_empty() || self.key.as_deref().unwrap_or("").contains(['=', '\n', '\r']) {
                    return Err("ini key must be a name without = or line breaks".to_string());
                }
                if self.section.as_deref().unwrap_or("").contains([']', '\n', '\r']) {
                    return Err("ini section must not contain ] or line breaks".to_string());
                }
                scalar(self.value.as_ref())?;
            }
            EditFormat::Xml => {
                let path = self.path.as_deref().unwrap_or("");
                if xml_segments(path).is_empty() {
                    return Err(format!("xml path {path:?} needs at least one element name (a/b or //b)"));
                }
                if let Some(a) = &self.attr {
                    if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "=\"'<>/".contains(c)) {
                        return Err(format!("xml attr {a:?} is not an attribute name"));
                    }
                }
                scalar(self.value.as_ref())?;
            }
            EditFormat::Json => {
                let p = self.pointer.as_deref().unwrap_or("");
                if !p.starts_with('/') {
                    return Err(format!("json pointer {p:?} must start with / (like /display/width)"));
                }
                if self.json_type.is_some() && !matches!(self.value, Some(serde_json::Value::String(_))) {
                    return Err("\"as\" only applies to a text value (a template such as \"$WIDTH\")".to_string());
                }
            }
            EditFormat::Text => {
                let replace_form = self.find.is_some() || self.replace.is_some();
                let line_form = self.starts_with.is_some() || self.set.is_some() || self.remove || self.add;
                if replace_form == line_form {
                    return Err("a text edit is either find + replace, or starts_with with set or remove".to_string());
                }
                if replace_form {
                    if self.find.as_deref().unwrap_or("").is_empty() || self.replace.is_none() {
                        return Err("a text replace needs a non-empty find and a replace".to_string());
                    }
                } else {
                    if self.starts_with.as_deref().unwrap_or("").is_empty() {
                        return Err("a text line edit needs a non-empty starts_with".to_string());
                    }
                    if self.set.is_some() == self.remove {
                        return Err("a text line edit needs exactly one of set and remove".to_string());
                    }
                    if self.add && self.set.is_none() {
                        return Err("add only goes with set".to_string());
                    }
                    if self.set.as_deref().is_some_and(|s| s.contains(['\n', '\r'])) {
                        return Err("set is one line".to_string());
                    }
                }
            }
        }
        Ok(())
    }
}

fn scalar(v: Option<&serde_json::Value>) -> Result<(), String> {
    match v {
        Some(serde_json::Value::String(s)) if !s.contains(['\n', '\r']) => Ok(()),
        Some(serde_json::Value::String(_)) => Err("value must be a single line".to_string()),
        Some(serde_json::Value::Number(_) | serde_json::Value::Bool(_)) => Ok(()),
        _ => Err("value must be text, a number or true/false".to_string()),
    }
}

fn collect_strings<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match v {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// The element names in an xml path: `a/b/c`, `/a/b` and `//c` all work (`//` lets the path start anywhere).
pub fn xml_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// True when an xml path may start at any depth (`//name`).
pub fn xml_anywhere(path: &str) -> bool {
    path.starts_with("//")
}

/// A settings file to change: a path relative to the copy, or one under `$USERPROFILE`, `$DOCUMENTS`,
/// `$APPDATA` or `$LOCALAPPDATA`. A file under a `files.saves` folder is edited in the seat's own save folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub file: String,
    pub format: EditFormat,
    /// Values may use seat variables and the file is edited again before each seat's launch. Without it the
    /// edit is the same for every seat and may not use them.
    #[serde(default, skip_serializing_if = "is_false")]
    pub per_seat: bool,
    /// A missing file is skipped instead of being an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// A missing file is created from nothing (ini, json and text).
    #[serde(default, skip_serializing_if = "is_false")]
    pub create: bool,
    #[serde(default, skip_serializing_if = "SeatSel::is_all")]
    pub seats: SeatSel,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
    pub set: Vec<Setting>,
}

impl Edit {
    pub fn validate(&self) -> Result<(), String> {
        check_file_template(&self.file)?;
        if self.set.is_empty() {
            return Err("set is empty".to_string());
        }
        if self.create && self.format == EditFormat::Xml {
            return Err("create does not work for xml".to_string());
        }
        if self.create && self.optional {
            return Err("use create or optional, not both".to_string());
        }
        for (i, s) in self.set.iter().enumerate() {
            s.validate(self.format).map_err(|e| format!("set[{i}]: {e}"))?;
        }
        Ok(())
    }
}

/// The machine variable a file template starts with (`$LOCALAPPDATA/...`), if it does.
pub fn machine_prefix(template: &str) -> Option<&'static str> {
    for name in crate::recipe::MACHINE_VARS {
        for pat in [format!("${name}"), format!("${{{name}}}")] {
            if let Some(rest) = template.strip_prefix(&pat) {
                if rest.is_empty() || rest.starts_with(['/', '\\']) {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// A path template that is relative to the copy or starts with a machine variable, and never climbs out.
pub fn check_file_template(file: &str) -> Result<(), String> {
    if file.is_empty() || file.contains('\0') {
        return Err(format!("bad file {file:?}"));
    }
    let tail = match machine_prefix(file) {
        Some(_) => file.split_once(['/', '\\']).map_or("", |(_, rest)| rest),
        None => {
            crate::recipe::check_relative(file)?;
            file
        }
    };
    if tail.split(['/', '\\']).any(|c| c == "..") {
        return Err(format!("{file:?} must not use .."));
    }
    Ok(())
}

// ---- per-seat files -----------------------------------------------------------------------------

/// A real file in each seat's copy: written from `content` (a template), or a fresh copy of the game's
/// file of the same name when `content` is left out. Never a link, so one seat cannot change another's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerSeatFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "SeatSel::is_all")]
    pub seats: SeatSel,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
}

// ---- patches ------------------------------------------------------------------------------------

/// Why a patch exists. Only these are accepted; Couchlink never strips DRM or anti-cheat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    Resolution,
    Windowed,
    Lan,
    Port,
    Focus,
    OtherSafe,
}

/// Bytes written as exactly one of `hex`, `text` (UTF-8), `utf16` (UTF-16 little endian text, as Windows
/// and Unity store strings) or, for a replacement only, `zeros: true` (as many zero bytes as the original).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bytes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utf16: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub zeros: bool,
}

impl Bytes {
    /// The bytes; `zeros` needs the length of the original.
    pub fn resolve(&self, original_len: Option<usize>) -> Result<Vec<u8>, String> {
        let n = [self.hex.is_some(), self.text.is_some(), self.utf16.is_some(), self.zeros].iter().filter(|b| **b).count();
        if n != 1 {
            return Err("give exactly one of hex, text, utf16 or zeros".to_string());
        }
        if let Some(h) = &self.hex {
            let h: String = h.chars().filter(|c| !c.is_whitespace()).collect();
            if h.len() % 2 != 0 || !h.is_ascii() {
                return Err(format!("bad hex {h:?}"));
            }
            return (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad hex {h:?}"))).collect();
        }
        if let Some(t) = &self.text {
            return Ok(t.as_bytes().to_vec());
        }
        if let Some(t) = &self.utf16 {
            return Ok(t.encode_utf16().flat_map(u16::to_le_bytes).collect());
        }
        original_len.map(|n| vec![0; n]).ok_or_else(|| "zeros is only for a replacement".to_string())
    }
}

/// Smallest and largest original pattern a patch may look for.
pub const PATCH_FIND_MIN: usize = 4;
pub const PATCH_FIND_MAX: usize = 4096;

/// A byte find and replace in one file of a seat's copy, for settings a game keeps inside its own files
/// (a resolution, a LAN switch, a port). The expected original bytes (and optionally the file's sha256) must
/// match or nothing is patched; the replacement has the same length; the real install is never touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub file: String,
    pub purpose: Purpose,
    pub find: Bytes,
    pub replace: Bytes,
    /// sha256 (hex) the original file must have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// How many times `find` must occur (all are replaced).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub count: u32,
    #[serde(default, skip_serializing_if = "SeatSel::is_all")]
    pub seats: SeatSel,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// Why a file may never be patched, if it may not: the game's own executable, Steam or EOS libraries,
/// and anti-cheat or DRM components. Couchlink never strips DRM or anti-cheat.
pub fn forbidden_patch_target(file: &str, main_exes: &[&str]) -> Option<&'static str> {
    let norm = |s: &str| s.replace('\\', "/").trim_matches('/').to_lowercase();
    let f = norm(file);
    if main_exes.iter().any(|e| !e.contains('$') && norm(e) == f) {
        return Some("the game's executable");
    }
    const BLOCKED: [(&str, &str); 22] = [
        ("steam_api", "a Steam library"),
        ("steamclient", "a Steam library"),
        ("eossdk", "an Epic Online Services library"),
        ("easyanticheat", "anti-cheat"),
        ("easy_anti_cheat", "anti-cheat"),
        ("eac_", "anti-cheat"),
        ("beclient", "anti-cheat"),
        ("beservice", "anti-cheat"),
        ("battleye", "anti-cheat"),
        ("battleeye", "anti-cheat"),
        ("equ8", "anti-cheat"),
        ("vanguard", "anti-cheat"),
        ("vgc", "anti-cheat"),
        ("xigncode", "anti-cheat"),
        ("gameguard", "anti-cheat"),
        ("punkbuster", "anti-cheat"),
        ("nprotect", "anti-cheat"),
        ("denuvo", "copy protection"),
        ("steamstub", "copy protection"),
        ("uplay_r", "a launcher library"),
        ("galaxy", "a launcher library"),
        ("onlinefix", "a copy protection bypass"),
    ];
    for part in f.split('/') {
        if let Some((_, why)) = BLOCKED.iter().find(|(p, _)| part.starts_with(p)) {
            return Some(why);
        }
    }
    None
}

impl Patch {
    pub fn validate(&self, main_exes: &[&str]) -> Result<(), String> {
        crate::recipe::check_relative(&self.file)?;
        if let Some(why) = forbidden_patch_target(&self.file, main_exes) {
            return Err(format!("{:?} is {why}; Couchlink does not patch it", self.file));
        }
        let find = self.find.resolve(None).map_err(|e| format!("find: {e}"))?;
        if self.find.zeros {
            return Err("find: zeros is only for a replacement".to_string());
        }
        if find.len() < PATCH_FIND_MIN || find.len() > PATCH_FIND_MAX {
            return Err(format!("find must be {PATCH_FIND_MIN} to {PATCH_FIND_MAX} bytes (it is {})", find.len()));
        }
        let replace = self.replace.resolve(Some(find.len())).map_err(|e| format!("replace: {e}"))?;
        if replace.len() != find.len() {
            return Err(format!("replace has {} bytes but find has {}; a patch keeps the length", replace.len(), find.len()));
        }
        if replace == find {
            return Err("replace is the same as find".to_string());
        }
        if let Some(h) = &self.sha256 {
            if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("sha256 must be 64 hex digits".to_string());
            }
        }
        if self.count == 0 || self.count > 1000 {
            return Err("count must be 1 to 1000".to_string());
        }
        Ok(())
    }
}

// ---- helpers ------------------------------------------------------------------------------------

/// When a helper starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperAt {
    /// `first`: before any copy. `per_seat_helpers`: before that seat's copy starts.
    #[default]
    Start,
    /// `first` only: once the first copy has its window.
    FirstCopy,
    /// `first`: once every copy has its window. `per_seat_helpers`: once that seat's copy has its window.
    Placed,
}

/// A program the session runs next to the game: a server, a proxy, a launcher script. Ended with the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Helper {
    pub exe: String,
    #[serde(default)]
    pub args: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Conditions to wait for after starting it, in order: `port:N`, `delay:MS`, `file:PATH`, `window:TEXT`
    /// (a window whose title has TEXT) or `exit` (the helper ends). One string or a list.
    #[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    pub wait: Vec<String>,
    #[serde(default, skip_serializing_if = "is_start")]
    pub at: HelperAt,
    #[serde(default, skip_serializing_if = "SeatSel::is_all")]
    pub seats: SeatSel,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
}

fn is_start(a: &HelperAt) -> bool {
    *a == HelperAt::Start
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seat_selectors() {
        assert!(SeatSel::parse("all").unwrap().matches(3));
        let from = SeatSel::parse("1+").unwrap();
        assert!(!from.matches(0) && from.matches(1) && from.matches(3));
        let list = SeatSel::parse("2, 0").unwrap();
        assert_eq!(list, SeatSel::List(vec![0, 2]));
        assert!(list.matches(2) && !list.matches(1));
        assert!(SeatSel::parse("4").is_err() && SeatSel::parse("x").is_err() && SeatSel::parse("").is_err());
        assert_eq!(serde_json::to_value(&from).unwrap(), json!("1+"));
        assert_eq!(serde_json::from_value::<SeatSel>(json!(2)).unwrap(), SeatSel::List(vec![2]));
        assert_eq!(serde_json::from_value::<SeatSel>(json!("0,2")).unwrap().to_string(), "0,2");
        assert!(serde_json::from_value::<SeatSel>(json!("9+")).is_err());
    }

    #[test]
    fn when_matches_options() {
        let w: When = serde_json::from_value(json!({"FPS": "Yes", "MODE": ["a", "b"]})).unwrap();
        let opts = |f: &str, m: &str| BTreeMap::from([("FPS".to_string(), f.to_string()), ("MODE".to_string(), m.to_string())]);
        assert!(w.holds(&opts("Yes", "b")));
        assert!(!w.holds(&opts("No", "b")) && !w.holds(&opts("Yes", "c")));
        assert!(When::default().holds(&BTreeMap::new()));
        assert_eq!(serde_json::to_value(&w).unwrap(), json!({"FPS": "Yes", "MODE": ["a", "b"]}));
    }

    #[test]
    fn options_resolve_and_validate() {
        let o: OptionDef = serde_json::from_value(json!({"label": "Mode", "choices": ["Old", {"value": "n", "label": "New"}], "default": "n"})).unwrap();
        assert_eq!(o.choices[0], Choice { value: "Old".into(), label: "Old".into() });
        assert_eq!(o.resolve(None).unwrap(), "n");
        assert_eq!(o.resolve(Some("Old")).unwrap(), "Old");
        assert!(o.resolve(Some("Newer")).unwrap_err().contains("not one of"));
        o.validate("MODE").unwrap();
        let first: OptionDef = serde_json::from_value(json!({"label": "L", "choices": ["a", "b"]})).unwrap();
        assert_eq!(first.default_value(), "a");
        let text: OptionDef = serde_json::from_value(json!({"label": "FOV", "default": "90"})).unwrap();
        assert!(text.is_text());
        assert_eq!(text.resolve(Some("100")).unwrap(), "100");
        assert!(text.resolve(Some("a\nb")).is_err() && text.resolve(Some(&"x".repeat(65))).is_err() && text.resolve(Some("$X")).is_err());
        let bad: OptionDef = serde_json::from_value(json!({"label": "L", "choices": ["a", "a"]})).unwrap();
        assert!(bad.validate("X").unwrap_err().contains("twice"));
        let bad: OptionDef = serde_json::from_value(json!({"label": "L", "choices": ["a"], "default": "z"})).unwrap();
        assert!(bad.validate("X").unwrap_err().contains("default"));
        let empty_choice: OptionDef = serde_json::from_value(json!({"label": "VR", "choices": [{"value": "", "label": "No"}, {"value": "-vr", "label": "Yes"}]})).unwrap();
        assert_eq!(empty_choice.default_value(), "");
        empty_choice.validate("VR").unwrap();
    }

    fn setting(v: serde_json::Value) -> Setting {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn settings_are_checked_per_format() {
        assert!(setting(json!({"section": "S", "key": "k", "value": 1})).validate(EditFormat::Ini).is_ok());
        assert!(setting(json!({"key": "k", "value": "v"})).validate(EditFormat::Ini).is_ok());
        assert!(setting(json!({"section": "S", "value": "v"})).validate(EditFormat::Ini).unwrap_err().contains("key"));
        assert!(setting(json!({"key": "k", "value": "v", "path": "a"})).validate(EditFormat::Ini).unwrap_err().contains("path"));
        assert!(setting(json!({"key": "k=1", "value": "v"})).validate(EditFormat::Ini).is_err());
        assert!(setting(json!({"key": "k", "value": "a\nb"})).validate(EditFormat::Ini).is_err());
        assert!(setting(json!({"key": "k", "value": ["a"]})).validate(EditFormat::Ini).is_err());
        assert!(setting(json!({"path": "//System", "attr": "Width", "value": "$WIDTH"})).validate(EditFormat::Xml).is_ok());
        assert!(setting(json!({"path": "//", "value": "1"})).validate(EditFormat::Xml).is_err());
        assert!(setting(json!({"path": "a", "attr": "x y", "value": "1"})).validate(EditFormat::Xml).is_err());
        assert!(setting(json!({"pointer": "/a/b", "value": {"x": 1}})).validate(EditFormat::Json).is_ok());
        assert!(setting(json!({"pointer": "a", "value": 1})).validate(EditFormat::Json).unwrap_err().contains("pointer"));
        assert!(setting(json!({"pointer": "/a", "value": 1, "as": "number"})).validate(EditFormat::Json).is_err());
        assert!(setting(json!({"pointer": "/a", "value": "$WIDTH", "as": "number"})).validate(EditFormat::Json).is_ok());
        assert!(setting(json!({"find": "a", "replace": "b"})).validate(EditFormat::Text).is_ok());
        assert!(setting(json!({"starts_with": "Res", "set": "Res = 1", "add": true})).validate(EditFormat::Text).is_ok());
        assert!(setting(json!({"starts_with": "Full", "remove": true})).validate(EditFormat::Text).is_ok());
        assert!(setting(json!({"starts_with": "Full", "remove": true, "set": "x"})).validate(EditFormat::Text).is_err());
        assert!(setting(json!({"find": "a", "replace": "b", "starts_with": "x", "remove": true})).validate(EditFormat::Text).is_err());
        assert!(setting(json!({"starts_with": "Full", "add": true, "remove": true})).validate(EditFormat::Text).is_err());
        assert!(setting(json!({})).validate(EditFormat::Text).is_err());
        assert!(serde_json::from_value::<Setting>(json!({"key": "k", "valeu": "x"})).is_err(), "unknown fields are refused");
    }

    #[test]
    fn edit_files_are_relative_or_machine_paths() {
        for ok in ["config/game.ini", "$USERPROFILE/AppData/Local/G/s.ini", "${DOCUMENTS}/My Games/x.xml", "$APPDATA\\G\\a.json"] {
            assert!(check_file_template(ok).is_ok(), "{ok}");
        }
        for bad in ["", "../x.ini", "/etc/x", "C:\\x.ini", "$USERPROFILE/../x"] {
            assert!(check_file_template(bad).is_err(), "{bad}");
        }
        assert_eq!(machine_prefix("$LOCALAPPDATA/x"), Some("LOCALAPPDATA"));
        assert_eq!(machine_prefix("$LOCALAPPDATAX/x"), None);
        assert_eq!(machine_prefix("game.ini"), None);
    }

    #[test]
    fn edits_validate() {
        let edit = |v: serde_json::Value| -> Result<(), String> { serde_json::from_value::<Edit>(v).map_err(|e| e.to_string())?.validate() };
        let ok = json!({"file": "a.ini", "format": "ini", "set": [{"section": "S", "key": "k", "value": "v"}]});
        assert!(edit(ok.clone()).is_ok());
        assert!(edit(json!({"file": "a.ini", "format": "ini", "set": []})).unwrap_err().contains("empty"));
        assert!(edit(json!({"file": "a.ini", "format": "yaml", "set": [{}]})).is_err());
        assert!(edit(json!({"file": "a.xml", "format": "xml", "create": true, "set": [{"path": "a", "value": "1"}]})).unwrap_err().contains("xml"));
        assert!(edit(json!({"file": "a.ini", "format": "ini", "create": true, "optional": true, "set": [{"key": "k", "value": "v"}]})).is_err());
        assert!(edit(json!({"file": "a.ini", "format": "ini", "set": [{"key": "k", "value": "v"}, {"key": "x"}]})).unwrap_err().contains("set[1]"));
        let e: Edit = serde_json::from_value(json!({"file": "a.ini", "format": "ini", "per_seat": true, "seats": "1+", "when": {"A": "b"}, "set": [{"key": "k", "value": "v"}]})).unwrap();
        assert_eq!(serde_json::from_value::<Edit>(serde_json::to_value(&e).unwrap()).unwrap(), e);
    }

    fn patch(v: serde_json::Value) -> Result<(), String> {
        serde_json::from_value::<Patch>(v).map_err(|e| e.to_string())?.validate(&["Game.exe", "bin/launcher.exe"])
    }

    #[test]
    fn patches_are_strictly_limited() {
        let ok = json!({"file": "data/UnityPlayer.dll", "purpose": "other-safe", "find": {"utf16": "Windows.Gaming.Input.Gamepad"}, "replace": {"zeros": true}});
        assert!(patch(ok.clone()).is_ok());
        let with = |k: &str, v: serde_json::Value| {
            let mut x = ok.clone();
            x[k] = v;
            x
        };
        for purpose in ["resolution", "windowed", "lan", "port", "focus", "other-safe"] {
            assert!(patch(with("purpose", json!(purpose))).is_ok(), "{purpose}");
        }
        for purpose in ["crack", "drm", "anticheat", "other", ""] {
            assert!(patch(with("purpose", json!(purpose))).is_err(), "{purpose}");
        }
        let mut no_purpose = ok.clone();
        no_purpose.as_object_mut().unwrap().remove("purpose");
        assert!(patch(no_purpose).unwrap_err().contains("purpose"));
        for file in ["Game.exe", "game.EXE", "bin\\launcher.exe", "steam_api64.dll", "bin/steam_api.dll", "EOSSDK-Win64-Shipping.dll", "EasyAntiCheat/x.dll", "x/EasyAntiCheat_EOS.dll", "BEClient_x64.dll", "denuvo64.dll", "../x.dll", "C:/x.dll"] {
            assert!(patch(with("file", json!(file))).is_err(), "{file}");
        }
        assert!(patch(with("file", json!("other.exe"))).is_ok(), "only the main executables are named");
        assert!(patch(with("find", json!({"hex": "0102"}))).unwrap_err().contains("bytes"));
        assert!(patch(with("find", json!({"hex": "01020304", "text": "x"}))).unwrap_err().contains("exactly one"));
        assert!(patch(with("find", json!({"zeros": true}))).is_err());
        assert!(patch(with("replace", json!({"text": "short"}))).unwrap_err().contains("keeps the length"));
        let same = json!({"file": "a.dat", "purpose": "port", "find": {"text": "abcd"}, "replace": {"text": "abcd"}});
        assert!(patch(same).unwrap_err().contains("same"));
        assert!(patch(with("sha256", json!("abc"))).is_err());
        assert!(patch(with("sha256", json!("a".repeat(64)))).is_ok());
        assert!(patch(with("count", json!(0))).is_err());
        let mut extra = ok.clone();
        extra["note"] = json!("x");
        assert!(patch(extra).is_err(), "unknown fields are refused");
    }

    #[test]
    fn bytes_resolve() {
        let b = |v: serde_json::Value| serde_json::from_value::<Bytes>(v).unwrap();
        assert_eq!(b(json!({"hex": "0a FF"})).resolve(None).unwrap(), vec![10, 255]);
        assert_eq!(b(json!({"text": "ab"})).resolve(None).unwrap(), b"ab".to_vec());
        assert_eq!(b(json!({"utf16": "ab"})).resolve(None).unwrap(), vec![b'a', 0, b'b', 0]);
        assert_eq!(b(json!({"zeros": true})).resolve(Some(3)).unwrap(), vec![0, 0, 0]);
        assert!(b(json!({"zeros": true})).resolve(None).is_err());
        assert!(b(json!({"hex": "0"})).resolve(None).is_err() && b(json!({})).resolve(None).is_err());
    }

    #[test]
    fn helpers_take_one_or_many_waits() {
        let h: Helper = serde_json::from_value(json!({"exe": "a.exe", "wait": "port:5"})).unwrap();
        assert_eq!(h.wait, vec!["port:5"]);
        let h: Helper = serde_json::from_value(json!({"exe": "a.exe", "wait": ["port:5", "delay:10"], "at": "first_copy", "seats": "0"})).unwrap();
        assert_eq!((h.wait.len(), h.at, h.seats.clone()), (2, HelperAt::FirstCopy, SeatSel::List(vec![0])));
        let h: Helper = serde_json::from_value(json!({"exe": "a.exe"})).unwrap();
        assert!(h.wait.is_empty() && h.at == HelperAt::Start);
        assert!(serde_json::from_value::<Helper>(json!({"exe": "a.exe", "at": "later"})).is_err());
    }
}
