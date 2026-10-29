//! A small Valve KeyValues (VDF text) parser. Enough for libraryfolders.vdf and appmanifest_*.acf.
//!
//! Supports quoted strings with `\\ \" \n \t` escapes, unquoted tokens, `{ }` nesting and `//` comments.
//! Duplicate keys are kept in order.

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VdfError(pub String);

impl std::fmt::Display for VdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VDF parse error: {}", self.0)
    }
}
impl std::error::Error for VdfError {}

impl Vdf {
    /// First child with this key (case-insensitive, as Steam treats keys).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(v) => v.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Vdf::as_str)
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(v) => v,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, VdfError> {
    let mut out = Vec::new();
    let mut it = src.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            '/' => {
                it.next();
                if it.peek() == Some(&'/') {
                    while let Some(&c) = it.peek() {
                        if c == '\n' {
                            break;
                        }
                        it.next();
                    }
                } else {
                    return Err(VdfError("stray '/'".into()));
                }
            }
            '{' => {
                it.next();
                out.push(Tok::Open);
            }
            '}' => {
                it.next();
                out.push(Tok::Close);
            }
            '"' => {
                it.next();
                let mut s = String::new();
                loop {
                    match it.next() {
                        None => return Err(VdfError("unterminated string".into())),
                        Some('"') => break,
                        Some('\\') => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('\\') => s.push('\\'),
                            Some('"') => s.push('"'),
                            Some(other) => {
                                s.push('\\');
                                s.push(other);
                            }
                            None => return Err(VdfError("unterminated escape".into())),
                        },
                        Some(ch) => s.push(ch),
                    }
                }
                out.push(Tok::Str(s));
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = it.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    it.next();
                }
                out.push(Tok::Str(s));
            }
        }
    }
    Ok(out)
}

fn parse_obj(toks: &[Tok], pos: &mut usize, top: bool) -> Result<Vec<(String, Vdf)>, VdfError> {
    let mut items = Vec::new();
    loop {
        match toks.get(*pos) {
            None => {
                return if top { Ok(items) } else { Err(VdfError("missing closing brace".into())) };
            }
            Some(Tok::Close) => {
                if top {
                    return Err(VdfError("unexpected closing brace".into()));
                }
                *pos += 1;
                return Ok(items);
            }
            Some(Tok::Open) => return Err(VdfError("unexpected opening brace".into())),
            Some(Tok::Str(key)) => {
                *pos += 1;
                match toks.get(*pos) {
                    Some(Tok::Str(val)) => {
                        *pos += 1;
                        items.push((key.clone(), Vdf::Str(val.clone())));
                    }
                    Some(Tok::Open) => {
                        *pos += 1;
                        let child = parse_obj(toks, pos, false)?;
                        items.push((key.clone(), Vdf::Obj(child)));
                    }
                    _ => return Err(VdfError(format!("key \"{key}\" has no value"))),
                }
            }
        }
    }
}

/// Parse a whole document. The result is always an object holding the top-level keys,
/// e.g. `parse(libraryfolders)?.get("libraryfolders")`.
pub fn parse(src: &str) -> Result<Vdf, VdfError> {
    let toks = tokenize(src)?;
    let mut pos = 0;
    Ok(Vdf::Obj(parse_obj(&toks, &mut pos, true)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_new_libraryfolders() {
        let src = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"228980"		"123"
			"250820"		"456"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}
"#;
        let v = parse(src).unwrap();
        let lf = v.get("LibraryFolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.get("0").unwrap().get_str("path"), Some(r"C:\Program Files (x86)\Steam"));
        assert_eq!(lf.get("1").unwrap().get_str("path"), Some(r"D:\SteamLibrary"));
        assert_eq!(lf.get("0").unwrap().get("apps").unwrap().entries().len(), 2);
    }

    #[test]
    fn comments_unquoted_and_escapes() {
        let src = "// head\n\"a\" { b c // trailing\n \"q\" \"x\\\"y\\ttab\" }\n";
        let v = parse(src).unwrap();
        assert_eq!(v.get("a").unwrap().get_str("b"), Some("c"));
        assert_eq!(v.get("a").unwrap().get_str("q"), Some("x\"y\ttab"));
    }

    #[test]
    fn duplicate_keys_kept_in_order() {
        let v = parse(r#""r" { "k" "1" "k" "2" }"#).unwrap();
        let r = v.get("r").unwrap();
        assert_eq!(r.entries().len(), 2);
        assert_eq!(r.entries()[1].1.as_str(), Some("2"));
    }

    #[test]
    fn rejects_broken_input() {
        assert!(parse(r#""a" { "b" "c""#).is_err());
        assert!(parse(r#""a" "#).is_err());
        assert!(parse(r#"}"#).is_err());
        assert!(parse(r#""abc"#).is_err());
    }

    #[test]
    fn bom_and_crlf() {
        let v = parse("\u{feff}\"AppState\"\r\n{\r\n\t\"appid\"\t\"264710\"\r\n}\r\n").unwrap();
        assert_eq!(v.get("appstate").unwrap().get_str("appid"), Some("264710"));
    }
}
