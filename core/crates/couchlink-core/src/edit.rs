//! Applying recipe `edits` to the text of a settings file (ini, xml, json, plain text) and `patches` to
//! bytes. Pure: text in, text out; the session runner does the file handling and journalling.
//! Edits keep everything they do not touch (comments, spacing, line endings) except json, which is
//! written back pretty printed with sorted keys.

use crate::recipe_parts::{xml_anywhere, xml_segments, EditFormat, JsonType, Setting};
use serde_json::Value;

/// How a file's text was stored, so it is written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

pub fn decode(bytes: &[u8]) -> Result<(String, Encoding), String> {
    let utf16 = |b: &[u8], le: bool| -> Result<String, String> {
        if b.len() % 2 != 0 {
            return Err("the file looks like UTF-16 but has an odd length".to_string());
        }
        let units: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16(&units).map_err(|_| "the file is not valid UTF-16 text".to_string())
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).map(|s| (s, Encoding::Utf8Bom)).map_err(|_| "the file is not valid UTF-8 text".to_string());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true).map(|s| (s, Encoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false).map(|s| (s, Encoding::Utf16Be));
    }
    String::from_utf8(bytes.to_vec()).map(|s| (s, Encoding::Utf8)).map_err(|_| "the file is not UTF-8 or UTF-16 text".to_string())
}

pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
        Encoding::Utf16Be => [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect(),
    }
}

/// Apply every setting to `text`. `expand` turns a template into its final text (the runner passes the
/// seat's variables); settings are applied in order.
pub fn apply(format: EditFormat, text: &str, settings: &[Setting], expand: &dyn Fn(&str) -> Result<String, String>) -> Result<String, String> {
    let mut text = text.to_string();
    for (i, s) in settings.iter().enumerate() {
        text = apply_one(format, &text, s, expand).map_err(|e| format!("set[{i}]: {e}"))?;
    }
    Ok(text)
}

fn apply_one(format: EditFormat, text: &str, s: &Setting, expand: &dyn Fn(&str) -> Result<String, String>) -> Result<String, String> {
    let ex = |o: &Option<String>| -> Result<String, String> { o.as_deref().map(expand).transpose().map(Option::unwrap_or_default) };
    let scalar = || -> Result<String, String> {
        match s.value.as_ref() {
            Some(Value::String(t)) => expand(t),
            Some(Value::Number(n)) => Ok(n.to_string()),
            Some(Value::Bool(b)) => Ok(b.to_string()),
            _ => Err("value must be text, a number or true/false".to_string()),
        }
    };
    match format {
        EditFormat::Ini => ini_set(text, &ex(&s.section)?, &ex(&s.key)?, &no_newline(scalar()?)?),
        EditFormat::Xml => xml_set(text, &ex(&s.path)?, s.attr.as_deref().map(expand).transpose()?.as_deref(), &scalar()?),
        EditFormat::Json => {
            let value = json_value(s.value.as_ref().unwrap_or(&Value::Null), expand, s.json_type)?;
            json_set(text, &ex(&s.pointer)?, value)
        }
        EditFormat::Text => {
            if let Some(find) = &s.find {
                text_replace(text, &expand(find)?, &ex(&s.replace)?)
            } else if s.remove {
                Ok(text_remove_lines(text, &ex(&s.starts_with)?))
            } else {
                text_set_line(text, &ex(&s.starts_with)?, &no_newline(ex(&s.set)?)?, s.add)
            }
        }
    }
}

fn no_newline(s: String) -> Result<String, String> {
    if s.contains(['\n', '\r']) {
        Err(format!("{s:?} has a line break"))
    } else {
        Ok(s)
    }
}

// ---- ini ----------------------------------------------------------------------------------------

fn split_lines(text: &str) -> (Vec<String>, bool, &'static str) {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    (text.lines().map(str::to_string).collect(), text.ends_with('\n'), eol)
}

fn join_lines(lines: &[String], trailing: bool, eol: &str) -> String {
    let mut out = lines.join(eol);
    if trailing && !lines.is_empty() {
        out.push_str(eol);
    }
    out
}

fn section_name(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn is_comment(line: &str) -> bool {
    matches!(line.trim_start().as_bytes().first(), Some(b';' | b'#'))
}

/// Set `key` in `[section]` (an empty section means the keys before the first header). An existing line keeps
/// its key text and spacing; a missing key is added at the end of its section; a missing section at the end.
pub fn ini_set(text: &str, section: &str, key: &str, value: &str) -> Result<String, String> {
    let (mut lines, mut trailing, eol) = split_lines(text);
    // (start, end) of the section's lines, end exclusive; the header itself is at start - 1 when named.
    let mut found: Option<(usize, usize)> = None;
    let mut current: Option<(usize, String)> = Some((0, String::new()));
    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(name) = section_name(l) {
            if let Some((start, n)) = current.take() {
                ranges.push((start, i, n));
            }
            current = Some((i + 1, name.to_string()));
        }
    }
    if let Some((start, n)) = current {
        ranges.push((start, lines.len(), n));
    }
    for (start, end, name) in &ranges {
        if name.eq_ignore_ascii_case(section) {
            found = Some((*start, *end));
            break;
        }
    }
    match found {
        Some((start, end)) => {
            for i in start..end {
                let l = &lines[i];
                if is_comment(l) {
                    continue;
                }
                let Some(eq) = l.find('=') else { continue };
                if l[..eq].trim().eq_ignore_ascii_case(key) {
                    let rest = &l[eq + 1..];
                    let ws = rest.len() - rest.trim_start().len();
                    lines[i] = format!("{}{}{}", &l[..=eq], &rest[..ws], value);
                    return Ok(join_lines(&lines, trailing || text.is_empty(), eol));
                }
            }
            // Insert after the last line of the section that holds something.
            let last = (start..end).rev().find(|i| !lines[*i].trim().is_empty());
            let at = last.map_or(start, |i| i + 1);
            lines.insert(at, format!("{key}={value}"));
        }
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            if !section.is_empty() {
                lines.push(format!("[{section}]"));
            }
            lines.push(format!("{key}={value}"));
            trailing = true;
        }
    }
    Ok(join_lines(&lines, trailing || text.is_empty(), eol))
}

// ---- xml ----------------------------------------------------------------------------------------

#[derive(Debug)]
struct Attr {
    name: String,
    value: (usize, usize),
    quote: u8,
}

#[derive(Debug)]
struct El {
    path: Vec<String>,
    /// `<` up to just past `>`.
    tag: (usize, usize),
    self_closing: bool,
    attrs: Vec<Attr>,
    /// Where a new attribute goes.
    attr_at: usize,
    /// Between the start and end tag.
    content: Option<(usize, usize)>,
    has_children: bool,
}

fn parse_xml(text: &str) -> Result<Vec<El>, String> {
    let b = text.as_bytes();
    let mut els: Vec<El> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0;
    let find = |from: usize, pat: &str| text[from..].find(pat).map(|p| from + p);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if rest.starts_with("<!--") {
            i = find(i + 4, "-->").ok_or("unclosed comment")? + 3;
        } else if rest.starts_with("<![CDATA[") {
            i = find(i + 9, "]]>").ok_or("unclosed CDATA")? + 3;
        } else if rest.starts_with("<?") {
            i = find(i + 2, "?>").ok_or("unclosed processing instruction")? + 2;
        } else if rest.starts_with("<!") {
            let mut depth = 0;
            let mut j = i + 2;
            while j < b.len() {
                match b[j] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
        } else if rest.starts_with("</") {
            let end = find(i, ">").ok_or("unclosed end tag")?;
            let name = text[i + 2..end].trim();
            let top = stack.pop().ok_or_else(|| format!("</{name}> closes nothing"))?;
            if els[top].path.last().map(String::as_str) != Some(name) {
                return Err(format!("</{name}> does not match <{}>", els[top].path.last().cloned().unwrap_or_default()));
            }
            if let Some(c) = els[top].content.as_mut() {
                c.1 = i;
            }
            i = end + 1;
        } else {
            let start = i;
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'/' && b[j] != b'>' {
                j += 1;
            }
            let name = text[i + 1..j].to_string();
            if name.is_empty() {
                return Err(format!("a stray < at byte {i}"));
            }
            let mut attrs = Vec::new();
            let mut attr_at = j;
            let self_closing;
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return Err(format!("<{name}> is never closed"));
                }
                if b[j] == b'>' {
                    self_closing = false;
                    break;
                }
                if b[j] == b'/' && b.get(j + 1) == Some(&b'>') {
                    self_closing = true;
                    break;
                }
                let ns = j;
                while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'=' && b[j] != b'>' && b[j] != b'/' {
                    j += 1;
                }
                let aname = text[ns..j].to_string();
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if aname.is_empty() || b.get(j) != Some(&b'=') {
                    return Err(format!("bad attribute in <{name}>"));
                }
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let q = *b.get(j).ok_or("unclosed attribute")?;
                if q != b'"' && q != b'\'' {
                    return Err(format!("attribute {aname} of <{name}> is not quoted"));
                }
                let vs = j + 1;
                let ve = text[vs..].find(q as char).ok_or("unclosed attribute value")? + vs;
                attrs.push(Attr { name: aname, value: (vs, ve), quote: q });
                j = ve + 1;
                attr_at = j;
            }
            let tag_end = if self_closing { j + 2 } else { j + 1 };
            if let Some(&p) = stack.last() {
                els[p].has_children = true;
            }
            let mut path = stack.last().map(|&p| els[p].path.clone()).unwrap_or_default();
            path.push(name);
            els.push(El {
                path,
                tag: (start, tag_end),
                self_closing,
                attrs,
                attr_at,
                content: (!self_closing).then_some((tag_end, tag_end)),
                has_children: false,
            });
            if !self_closing {
                stack.push(els.len() - 1);
            }
            i = tag_end;
        }
    }
    if let Some(&open) = stack.last() {
        return Err(format!("<{}> is never closed", els[open].path.last().cloned().unwrap_or_default()));
    }
    if els.is_empty() {
        return Err("no elements".to_string());
    }
    Ok(els)
}

fn xml_escape(s: &str, quote: Option<u8>) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' if quote == Some(b'"') => o.push_str("&quot;"),
            '\'' if quote == Some(b'\'') => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

/// Set an attribute (`attr` given) or the text of every element `path` selects. `a/b/c` starts at the root
/// element, `//b/c` anywhere. An element must exist; nothing is created.
pub fn xml_set(text: &str, path: &str, attr: Option<&str>, value: &str) -> Result<String, String> {
    let segs = xml_segments(path);
    if segs.is_empty() {
        return Err(format!("xml path {path:?} is empty"));
    }
    let els = parse_xml(text).map_err(|e| format!("cannot read the xml: {e}"))?;
    let anywhere = xml_anywhere(path);
    let mut hits: Vec<&El> = els
        .iter()
        .filter(|e| if anywhere { e.path.len() >= segs.len() && e.path[e.path.len() - segs.len()..] == segs[..] } else { e.path == segs })
        .collect();
    if hits.is_empty() {
        return Err(format!("no element {path} in the file"));
    }
    hits.sort_by_key(|e| std::cmp::Reverse(e.tag.0));
    let mut out = text.to_string();
    for e in hits {
        match attr {
            Some(a) => match e.attrs.iter().find(|x| x.name == a) {
                Some(x) => out.replace_range(x.value.0..x.value.1, &xml_escape(value, Some(x.quote))),
                None => out.insert_str(e.attr_at, &format!(" {a}=\"{}\"", xml_escape(value, Some(b'"')))),
            },
            None => {
                if e.has_children {
                    return Err(format!("{path} has child elements, so its text cannot be set"));
                }
                let name = e.path.last().expect("path is never empty");
                match e.content {
                    Some((s, t)) => out.replace_range(s..t, &xml_escape(value, None)),
                    None => {
                        // <name a="1"/> becomes <name a="1">value</name>
                        let open = out[e.tag.0..e.tag.1 - 2].trim_end().to_string();
                        out.replace_range(e.tag.0..e.tag.1, &format!("{open}>{}</{name}>", xml_escape(value, None)));
                    }
                }
            }
        }
        let _ = e.self_closing;
    }
    Ok(out)
}

// ---- json ---------------------------------------------------------------------------------------

/// A json value from a recipe: strings are templates (expanded, then stored as `as` says), other values as they are.
pub fn json_value(v: &Value, expand: &dyn Fn(&str) -> Result<String, String>, as_type: Option<JsonType>) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => {
            let t = expand(s)?;
            match as_type {
                None | Some(JsonType::String) => Value::String(t),
                Some(JsonType::Number) => {
                    let t = t.trim();
                    t.parse::<i64>()
                        .map(Value::from)
                        .or_else(|_| t.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).ok_or(()))
                        .map_err(|_| format!("{t:?} is not a number"))?
                }
                Some(JsonType::Bool) => match t.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" => Value::Bool(true),
                    "false" | "0" | "no" => Value::Bool(false),
                    _ => return Err(format!("{t:?} is not true or false")),
                },
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| json_value(x, expand, None)).collect::<Result<_, _>>()?),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), json_value(x, expand, None)?))).collect::<Result<_, String>>()?),
        other => other.clone(),
    })
}

/// Set the value at a json pointer (`/a/b/0`), creating missing objects on the way.
pub fn json_set(text: &str, pointer: &str, value: Value) -> Result<String, String> {
    let mut root: Value = if text.trim().is_empty() { Value::Object(Default::default()) } else { serde_json::from_str(text).map_err(|e| format!("cannot read the json: {e}"))? };
    let tokens: Vec<String> = pointer.strip_prefix('/').ok_or("pointer must start with /")?.split('/').map(|t| t.replace("~1", "/").replace("~0", "~")).collect();
    let mut cur = &mut root;
    for (n, tok) in tokens.iter().enumerate() {
        let last = n + 1 == tokens.len();
        match cur {
            Value::Object(map) => {
                if last {
                    map.insert(tok.clone(), value);
                    break;
                }
                cur = map.entry(tok.clone()).or_insert_with(|| Value::Object(Default::default()));
            }
            Value::Array(arr) => {
                let idx = if tok == "-" { arr.len() } else { tok.parse::<usize>().map_err(|_| format!("{tok:?} is not an array index"))? };
                if idx > arr.len() || (idx == arr.len() && !last) {
                    return Err(format!("index {idx} is past the end of the array"));
                }
                if last {
                    if idx == arr.len() {
                        arr.push(value);
                    } else {
                        arr[idx] = value;
                    }
                    break;
                }
                cur = &mut arr[idx];
            }
            _ => return Err(format!("{tok:?}: the value above it is neither an object nor an array")),
        }
    }
    let mut out = serde_json::to_string_pretty(&root).expect("serialise json");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

// ---- plain text ---------------------------------------------------------------------------------

/// Replace every `find`. When it is gone but `replace` is already there, the file is as wanted and stays.
pub fn text_replace(text: &str, find: &str, replace: &str) -> Result<String, String> {
    if text.contains(find) {
        Ok(text.replace(find, replace))
    } else if !replace.is_empty() && text.contains(replace) {
        Ok(text.to_string())
    } else {
        Err(format!("{find:?} is not in the file"))
    }
}

/// Replace the first line that starts with `starts_with`; add it at the end when there is none and `add` is set.
pub fn text_set_line(text: &str, starts_with: &str, line: &str, add: bool) -> Result<String, String> {
    let (mut lines, trailing, eol) = split_lines(text);
    match lines.iter().position(|l| l.trim_start().starts_with(starts_with)) {
        Some(i) => lines[i] = line.to_string(),
        None if add => lines.push(line.to_string()),
        None => return Err(format!("no line starts with {starts_with:?}")),
    }
    Ok(join_lines(&lines, trailing || text.is_empty() || add, eol))
}

pub fn text_remove_lines(text: &str, starts_with: &str) -> String {
    let (lines, trailing, eol) = split_lines(text);
    let kept: Vec<String> = lines.into_iter().filter(|l| !l.trim_start().starts_with(starts_with)).collect();
    join_lines(&kept, trailing, eol)
}

// ---- byte patches -------------------------------------------------------------------------------

/// Replace every occurrence of `find` with `replace` (same length), but only if `find` occurs exactly
/// `count` times. Anything else means the file is not the version the recipe was written for.
pub fn apply_patch(data: &[u8], find: &[u8], replace: &[u8], count: usize) -> Result<Vec<u8>, String> {
    if find.is_empty() || find.len() != replace.len() {
        return Err("the patch must replace bytes with the same number of bytes".to_string());
    }
    let mut out = data.to_vec();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + find.len() <= data.len() {
        if &data[i..i + find.len()] == find {
            hits.push(i);
            i += find.len();
        } else {
            i += 1;
        }
    }
    if hits.len() != count {
        return Err(format!("found the original bytes {} time(s) but the recipe expects {count}; this is a different version of the file, so it is left alone", hits.len()));
    }
    for h in hits {
        out[h..h + find.len()].copy_from_slice(replace);
    }
    Ok(out)
}

// ---- sha256 -------------------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7,
    0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// sha256 of `data` as lower case hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ident(s: &str) -> Result<String, String> {
        Ok(s.to_string())
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(sha256_hex(&vec![b'a'; 1000]).len(), 64);
    }

    #[test]
    fn text_encodings_round_trip() {
        for (bytes, enc) in [
            (b"a=1\r\n".to_vec(), Encoding::Utf8),
            ([&[0xEF, 0xBB, 0xBF][..], b"a=1"].concat(), Encoding::Utf8Bom),
            (encode("a=1\n", Encoding::Utf16Le), Encoding::Utf16Le),
            (encode("a=1\n", Encoding::Utf16Be), Encoding::Utf16Be),
        ] {
            let (text, e) = decode(&bytes).unwrap();
            assert_eq!(e, enc);
            assert_eq!(encode(&text, e), bytes);
        }
        assert!(decode(&[0xFF, 0xFE, 0x61]).is_err() && decode(&[0xC3, 0x28]).is_err());
    }

    #[test]
    fn ini_changes_one_value_and_keeps_the_rest() {
        let t = "; settings\n[Display]\nWidth = 800\nHeight=600\n\n[Audio]\nVolume=5\n";
        let out = ini_set(t, "Display", "width", "1920").unwrap();
        assert_eq!(out, "; settings\n[Display]\nWidth = 1920\nHeight=600\n\n[Audio]\nVolume=5\n");
        assert_eq!(ini_set(t, "display", "Height", "1080").unwrap(), "; settings\n[Display]\nWidth = 800\nHeight=1080\n\n[Audio]\nVolume=5\n");
        // A key in the wrong section is not touched.
        let out = ini_set(t, "Audio", "Width", "1").unwrap();
        assert_eq!(out, "; settings\n[Display]\nWidth = 800\nHeight=600\n\n[Audio]\nVolume=5\nWidth=1\n");
        // A comment line that looks like the key is skipped.
        assert_eq!(ini_set("[S]\n;k=1\nk=2\n", "S", "k", "9").unwrap(), "[S]\n;k=1\nk=9\n");
    }

    #[test]
    fn ini_adds_keys_sections_and_keeps_line_endings() {
        let t = "[A]\r\nx=1\r\n\r\n[B]\r\ny=2\r\n";
        let out = ini_set(t, "A", "z", "3").unwrap();
        assert_eq!(out, "[A]\r\nx=1\r\nz=3\r\n\r\n[B]\r\ny=2\r\n");
        let out = ini_set(t, "C", "k", "v").unwrap();
        assert_eq!(out, "[A]\r\nx=1\r\n\r\n[B]\r\ny=2\r\n\r\n[C]\r\nk=v\r\n");
        assert_eq!(ini_set("", "S", "k", "v").unwrap(), "[S]\nk=v\n");
        assert_eq!(ini_set("", "", "k", "v").unwrap(), "k=v\n");
        // Global keys come before the first header.
        assert_eq!(ini_set("[S]\na=1\n", "", "g", "1").unwrap(), "g=1\n[S]\na=1\n");
        assert_eq!(ini_set("g=0\n[S]\na=1\n", "", "g", "1").unwrap(), "g=1\n[S]\na=1\n");
        // No trailing newline stays that way when a value changes.
        assert_eq!(ini_set("[S]\nk=1", "S", "k", "2").unwrap(), "[S]\nk=2");
        // Unreal style keys.
        assert_eq!(ini_set("[/Script/X.Y]\nbLumen=True\n+Arr=1\n", "/Script/X.Y", "bLumen", "False").unwrap(), "[/Script/X.Y]\nbLumen=False\n+Arr=1\n");
    }

    #[test]
    fn xml_sets_attributes_and_text() {
        let t = "<?xml version=\"1.0\"?>\n<!-- c -->\n<Config>\n  <System Width=\"800\" Height='600' />\n  <Windowed>0</Windowed>\n  <Empty/>\n</Config>\n";
        let out = xml_set(t, "//System", Some("Width"), "1920").unwrap();
        assert!(out.contains("<System Width=\"1920\" Height='600' />"), "{out}");
        let out = xml_set(&out, "Config/System", Some("Height"), "1080").unwrap();
        assert!(out.contains("Height='1080'"), "{out}");
        let out = xml_set(&out, "//System", Some("Mode"), "a&b").unwrap();
        assert!(out.contains("<System Width=\"1920\" Height='1080' Mode=\"a&amp;b\" />"), "{out}");
        let out = xml_set(&out, "//Windowed", None, "1").unwrap();
        assert!(out.contains("<Windowed>1</Windowed>"));
        let out = xml_set(&out, "/Config/Empty", None, "x<y").unwrap();
        assert!(out.contains("<Empty>x&lt;y</Empty>"), "{out}");
        let out = xml_set(&out, "//Empty", Some("id"), "7").unwrap();
        assert!(out.contains("<Empty id=\"7\">"), "{out}");
        assert!(out.starts_with("<?xml version=\"1.0\"?>\n<!-- c -->\n<Config>"), "everything else is kept");
        assert!(xml_set(t, "//Nope", None, "1").unwrap_err().contains("no element"));
        assert!(xml_set(t, "System", Some("Width"), "1").is_err(), "a path without // starts at the root");
        assert!(xml_set(t, "//Config", None, "x").unwrap_err().contains("child elements"));
        assert!(xml_set("<a><b></a>", "//b", None, "1").is_err());
        assert!(xml_set("not xml", "//a", None, "1").is_err());
        // Several matches are all changed.
        let many = "<r><i v=\"1\"/><i v=\"2\"/></r>";
        assert_eq!(xml_set(many, "//i", Some("v"), "9").unwrap(), "<r><i v=\"9\"/><i v=\"9\"/></r>");
        assert_eq!(xml_set("<r><i><v>1</v></i></r>", "i/v", None, "2").is_err(), true);
        assert_eq!(xml_set("<r><i><v>1</v></i></r>", "r/i/v", None, "2").unwrap(), "<r><i><v>2</v></i></r>");
    }

    #[test]
    fn json_sets_by_pointer() {
        let t = "{\"a\": {\"b\": 1}, \"list\": [1, 2]}\n";
        let out = json_set(t, "/a/b", json!(5)).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, json!({"a": {"b": 5}, "list": [1, 2]}));
        assert!(out.ends_with('\n'));
        let v: Value = serde_json::from_str(&json_set(t, "/x/y/z", json!("n")).unwrap()).unwrap();
        assert_eq!(v["x"]["y"]["z"], "n");
        let v: Value = serde_json::from_str(&json_set(t, "/list/1", json!(9)).unwrap()).unwrap();
        assert_eq!(v["list"], json!([1, 9]));
        let v: Value = serde_json::from_str(&json_set(t, "/list/-", json!(3)).unwrap()).unwrap();
        assert_eq!(v["list"], json!([1, 2, 3]));
        let v: Value = serde_json::from_str(&json_set("", "/k", json!(true)).unwrap()).unwrap();
        assert_eq!(v, json!({"k": true}));
        let v: Value = serde_json::from_str(&json_set(t, "/a~1b", json!(1)).unwrap()).unwrap();
        assert_eq!(v["a/b"], 1);
        assert!(json_set(t, "/list/5", json!(1)).is_err() && json_set(t, "/a/b/c", json!(1)).is_err() && json_set("{", "/a", json!(1)).is_err());
    }

    #[test]
    fn json_values_follow_their_type() {
        let ex = |s: &str| Ok(s.replace("$WIDTH", "1920"));
        assert_eq!(json_value(&json!("$WIDTH"), &ex, Some(JsonType::Number)).unwrap(), json!(1920));
        assert_eq!(json_value(&json!("$WIDTH"), &ex, None).unwrap(), json!("1920"));
        assert_eq!(json_value(&json!("1.5"), &ex, Some(JsonType::Number)).unwrap(), json!(1.5));
        assert_eq!(json_value(&json!("yes"), &ex, Some(JsonType::Bool)).unwrap(), json!(true));
        assert_eq!(json_value(&json!({"w": ["$WIDTH", 2]}), &ex, None).unwrap(), json!({"w": ["1920", 2]}));
        assert!(json_value(&json!("x"), &ex, Some(JsonType::Number)).is_err());
        assert!(json_value(&json!("x"), &ex, Some(JsonType::Bool)).is_err());
    }

    #[test]
    fn text_edits() {
        assert_eq!(text_replace("a b a", "a", "c").unwrap(), "c b c");
        assert_eq!(text_replace("c b", "a", "c").unwrap(), "c b", "already applied");
        assert!(text_replace("x", "a", "c").is_err());
        assert!(text_replace("x", "a", "").is_err());
        assert_eq!(text_set_line("A=1\nRes=1\nB=2\n", "Res", "Res=2", false).unwrap(), "A=1\nRes=2\nB=2\n");
        assert!(text_set_line("A=1\n", "Res", "Res=2", false).is_err());
        assert_eq!(text_set_line("A=1\n", "Res", "Res=2", true).unwrap(), "A=1\nRes=2\n");
        assert_eq!(text_remove_lines("A\r\nFull=1\r\nB\r\n", "Full"), "A\r\nB\r\n");
        assert_eq!(text_remove_lines("A\n", "Full"), "A\n");
    }

    #[test]
    fn settings_apply_in_order_with_templates() {
        let ex = |s: &str| Ok(s.replace("$WIDTH", "1280").replace("$HEIGHT", "720"));
        let s = |v: Value| -> Setting { serde_json::from_value(v).unwrap() };
        let ini = apply(
            EditFormat::Ini,
            "[Display]\nResolution_Width=1\n",
            &[s(json!({"section": "Display", "key": "Resolution_Width", "value": "$WIDTH"})), s(json!({"section": "Display", "key": "Resolution_Height", "value": "$HEIGHT"}))],
            &ex,
        )
        .unwrap();
        assert_eq!(ini, "[Display]\nResolution_Width=1280\nResolution_Height=720\n");
        let x = apply(EditFormat::Xml, "<a w=\"1\"/>", &[s(json!({"path": "//a", "attr": "w", "value": "$WIDTH"}))], &ex).unwrap();
        assert_eq!(x, "<a w=\"1280\"/>");
        let j = apply(EditFormat::Json, "{}", &[s(json!({"pointer": "/w", "value": "$WIDTH", "as": "number"})), s(json!({"pointer": "/on", "value": true}))], &ex).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&j).unwrap(), json!({"w": 1280, "on": true}));
        let t = apply(EditFormat::Text, "ScreenWidth = 1\nFullScreen = true\n", &[s(json!({"starts_with": "ScreenWidth", "set": "ScreenWidth = $WIDTH"})), s(json!({"starts_with": "FullScreen", "remove": true}))], &ex).unwrap();
        assert_eq!(t, "ScreenWidth = 1280\n");
        let e = apply(EditFormat::Text, "x", &[s(json!({"find": "a", "replace": "b"}))], &ident).unwrap_err();
        assert!(e.starts_with("set[0]"), "{e}");
        assert!(apply(EditFormat::Ini, "", &[s(json!({"key": "k", "value": "a\nb"}))], &ident).is_err());
    }

    #[test]
    fn patches_need_the_exact_original() {
        let data = b"....Windows.Gaming....Windows.Gaming....".to_vec();
        let out = apply_patch(&data, b"Windows.Gaming", b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0", 2).unwrap();
        assert_eq!(out.len(), data.len());
        assert!(!out.windows(7).any(|w| w == b"Windows"));
        assert_eq!(&out[..4], b"....");
        assert!(apply_patch(&data, b"Windows.Gaming", &[0; 14], 1).unwrap_err().contains("2 time"));
        assert!(apply_patch(&data, b"Nothing", &[0; 7], 1).unwrap_err().contains("0 time"));
        assert!(apply_patch(&data, b"abcd", b"abc", 1).is_err());
    }
}
