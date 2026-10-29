//! A tolerant extractor for the JavaScript of SplitScreen.Me / Nucleus handlers.
//!
//! There is no JS engine here. A handler is mostly a list of `Game.X = value;` assignments plus a
//! `Game.Play = function () { ... }` body. This module lexes the text, then walks statements and
//! records two kinds of events: assignments (`a.b.c = value`, `+=`) and calls (`a.b(args)`),
//! each tagged with the function it sits in and its block depth. Anything it does not understand is
//! skipped, never an error: odd JS must not stop an import.

use std::collections::{BTreeMap, BTreeSet};

/// A value read from the script. Only literal shapes are evaluated; everything else is kept as
/// `Ref` (a dotted name such as `Context.Width`) or `Other` (calls, ternaries, objects, ...).
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    /// A `+` chain that mixes literals and references, for example `"-w " + Context.Width`.
    Concat(Vec<Val>),
    /// An unresolved dotted name.
    Ref(String),
    /// A function expression (the body is walked separately).
    Func,
    /// Anything else; the text is a short rendering for notes.
    Other(String),
}

impl Val {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Val::Bool(b) => Some(*b),
            Val::Num(n) => Some(*n != 0.0),
            Val::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    /// String items of an array; non-string items are rendered. A single string becomes a
    /// one-item list.
    pub fn as_str_list(&self) -> Option<Vec<String>> {
        match self {
            Val::Arr(items) => Some(
                items
                    .iter()
                    .map(|v| match v {
                        Val::Str(s) => s.clone(),
                        other => other.display(),
                    })
                    .collect(),
            ),
            Val::Str(s) => Some(vec![s.clone()]),
            _ => None,
        }
    }

    /// True when the value is made only of literals (no references or unknowns).
    pub fn is_literal(&self) -> bool {
        match self {
            Val::Null | Val::Bool(_) | Val::Num(_) | Val::Str(_) => true,
            Val::Arr(items) => items.iter().all(Val::is_literal),
            _ => false,
        }
    }

    /// Human readable rendering used in notes: references appear as `<Context.Width>`.
    pub fn display(&self) -> String {
        match self {
            Val::Null => "null".into(),
            Val::Bool(b) => b.to_string(),
            Val::Num(n) => num_to_string(*n),
            Val::Str(s) => s.clone(),
            Val::Arr(items) => {
                let inner: Vec<String> = items.iter().map(Val::display).collect();
                format!("[{}]", inner.join(", "))
            }
            Val::Concat(parts) => parts.iter().map(Val::display).collect::<String>(),
            Val::Ref(r) => format!("<{r}>"),
            Val::Func => "<function>".into(),
            Val::Other(t) => format!("<{t}>"),
        }
    }
}

pub(crate) fn num_to_string(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ---------------------------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    /// Punctuation or operator, possibly several characters (`+=`, `===`, `&&`).
    P(String),
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.trim_start_matches('\u{feff}').chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut out: Vec<Tok> = Vec::new();

    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        // strings and template literals
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            while i < n {
                let d = chars[i];
                if d == '\\' && i + 1 < n {
                    let e = chars[i + 1];
                    i += 2;
                    match e {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '0' => s.push('\0'),
                        'b' => s.push('\u{8}'),
                        'f' => s.push('\u{c}'),
                        'v' => s.push('\u{b}'),
                        'x' => {
                            let hex: String = chars.iter().skip(i).take(2).collect();
                            if let Ok(v) = u32::from_str_radix(&hex, 16) {
                                if let Some(ch) = char::from_u32(v) {
                                    s.push(ch);
                                }
                                i += 2;
                            }
                        }
                        'u' => {
                            let hex: String = chars.iter().skip(i).take(4).collect();
                            if let Ok(v) = u32::from_str_radix(&hex, 16) {
                                if let Some(ch) = char::from_u32(v) {
                                    s.push(ch);
                                }
                                i += 4;
                            }
                        }
                        '\r' => {
                            if i < n && chars[i] == '\n' {
                                i += 1;
                            }
                        }
                        '\n' => {}
                        other => s.push(other),
                    }
                    continue;
                }
                if d == quote {
                    i += 1;
                    break;
                }
                // An unterminated plain string ends at the line break instead of eating the file.
                if d == '\n' && quote != '`' {
                    break;
                }
                s.push(d);
                i += 1;
            }
            out.push(Tok::Str(s));
            continue;
        }
        // numbers
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && chars[i + 1].is_ascii_digit()) {
            let start = i;
            if c == '0' && i + 1 < n && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                i += 2;
                let hs = i;
                while i < n && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex: String = chars[hs..i].iter().collect();
                out.push(Tok::Num(u64::from_str_radix(&hex, 16).unwrap_or(0) as f64));
                continue;
            }
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < n && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < n && chars[j].is_ascii_digit() {
                    i = j;
                    while i < n && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Tok::Num(text.parse().unwrap_or(0.0)));
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident_part(chars[i]) {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        // regex literal: a slash where a value cannot precede it
        if c == '/' {
            let value_before = matches!(
                out.last(),
                Some(Tok::Ident(_)) | Some(Tok::Num(_)) | Some(Tok::Str(_))
            ) || matches!(out.last(), Some(Tok::P(p)) if p == ")" || p == "]");
            if !value_before {
                let mut j = i + 1;
                let mut in_class = false;
                let mut closed = false;
                while j < n && chars[j] != '\n' {
                    match chars[j] {
                        '\\' => j += 1,
                        '[' => in_class = true,
                        ']' => in_class = false,
                        '/' if !in_class => {
                            closed = true;
                            break;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if closed {
                    i = j + 1;
                    while i < n && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    out.push(Tok::Str(String::new()));
                    continue;
                }
            }
        }
        // operators
        let three: String = chars[i..(i + 3).min(n)].iter().collect();
        let two: String = chars[i..(i + 2).min(n)].iter().collect();
        if three == "===" || three == "!==" {
            out.push(Tok::P(three));
            i += 3;
        } else if matches!(
            two.as_str(),
            "==" | "!=" | "<=" | ">=" | "&&" | "||" | "+=" | "-=" | "++" | "--" | "=>"
        ) {
            out.push(Tok::P(two));
            i += 2;
        } else {
            out.push(Tok::P(c.to_string()));
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// `path = value`
    Assign(Val),
    /// `path += value`
    AddAssign(Val),
    /// `path(args)`; `new X(args)` is recorded as path `new X`.
    Call(Vec<Val>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub path: String,
    pub kind: EventKind,
    /// Path of the function this statement sits in (`Game.Play`), `None` at top level.
    pub func: Option<String>,
    /// 0 at top level; 1 directly in a function body; more inside if/for/while blocks.
    pub depth: usize,
}

/// Result of scanning a handler script.
#[derive(Debug, Clone, Default)]
pub struct ParsedHandler {
    /// Top-level `Game.X = value` (key without the `Game.` prefix), with unconditional
    /// assignments made directly in `Game.Play` applied on top.
    pub fields: BTreeMap<String, Val>,
    /// Top-level `Hub.X = value` (`Handler.Id`, `Handler.Version`, `Maintainer.Name`, ...).
    pub hub: BTreeMap<String, Val>,
    /// `Game.AddOption(title, description, key, values)` calls.
    pub options: Vec<UserOption>,
    /// Paths assigned a function (`Game.Play`, `Game.ProtoInput.OnInputLocked`, ...).
    pub functions: BTreeSet<String>,
    /// Keys (without `Game.`) assigned inside nested blocks of `Game.Play`, where the value
    /// depends on runtime conditions.
    pub conditional: BTreeMap<String, Vec<Val>>,
    /// Every call made inside `Game.Play` (any depth) as `(path, args, depth)`.
    pub play_calls: Vec<(String, Vec<Val>, usize)>,
    /// `Context.X = value` assignments inside `Game.Play` as `(name, op_is_append, value, depth)`.
    pub play_context_sets: Vec<(String, bool, Val, usize)>,
    /// Number of calls and assignments recorded inside `Game.Play` (any depth).
    pub play_statements: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserOption {
    pub title: String,
    pub description: String,
    pub key: String,
    pub values: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    vars: BTreeMap<String, Val>,
    events: Vec<Event>,
    func: Option<String>,
    depth: usize,
    /// Guards runaway recursion on hostile input.
    nest: usize,
}

const MAX_NEST: usize = 200;

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, off: usize) -> Option<&Tok> {
        self.toks.get(self.pos + off)
    }

    fn eof(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn is_p(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::P(x)) if x == p)
    }

    fn is_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(x)) if x == name)
    }

    fn eat_p(&mut self, p: &str) -> bool {
        if self.is_p(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn emit(&mut self, path: String, kind: EventKind) {
        self.events.push(Event { path, kind, func: self.func.clone(), depth: self.depth });
    }

    /// Skip a balanced group starting at the current open bracket token. Returns the rendered
    /// text of the first few tokens inside, for notes.
    fn skip_group(&mut self) -> String {
        let (open, close) = match self.peek() {
            Some(Tok::P(p)) if p == "(" => ("(", ")"),
            Some(Tok::P(p)) if p == "[" => ("[", "]"),
            Some(Tok::P(p)) if p == "{" => ("{", "}"),
            _ => return String::new(),
        };
        let mut level = 0usize;
        let mut text = String::new();
        while let Some(t) = self.peek().cloned() {
            self.pos += 1;
            match &t {
                Tok::P(p) if p == open => level += 1,
                Tok::P(p) if p == close => {
                    level = level.saturating_sub(1);
                    if level == 0 {
                        break;
                    }
                }
                _ => {}
            }
            if text.len() < 40 {
                match &t {
                    Tok::Ident(s) => text.push_str(s),
                    Tok::Str(s) => text.push_str(&format!("\"{s}\"")),
                    Tok::Num(n) => text.push_str(&num_to_string(*n)),
                    Tok::P(p) => text.push_str(p),
                }
            }
        }
        text
    }

    // ----- expressions -----

    fn parse_expr(&mut self) -> Val {
        if self.nest > MAX_NEST {
            return Val::Other("too deep".into());
        }
        self.nest += 1;
        let v = self.parse_ternary();
        self.nest -= 1;
        v
    }

    fn parse_ternary(&mut self) -> Val {
        let cond = self.parse_binary();
        if self.is_p("?") {
            self.pos += 1;
            self.parse_ternary();
            if self.eat_p(":") {
                self.parse_ternary();
            }
            return Val::Other("conditional".into());
        }
        cond
    }

    fn parse_binary(&mut self) -> Val {
        let mut left = self.parse_additive();
        loop {
            let op = match self.peek() {
                Some(Tok::P(p))
                    if matches!(
                        p.as_str(),
                        "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "*"
                            | "/" | "%" | "-" | "&" | "|" | "^"
                    ) =>
                {
                    p.clone()
                }
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_additive();
            left = match (op.as_str(), &left, &right) {
                ("-", Val::Num(a), Val::Num(b)) => Val::Num(a - b),
                ("*", Val::Num(a), Val::Num(b)) => Val::Num(a * b),
                _ => Val::Other("expression".into()),
            };
        }
        left
    }

    fn parse_additive(&mut self) -> Val {
        let mut left = self.parse_unary();
        while self.is_p("+") {
            self.pos += 1;
            let right = self.parse_unary();
            left = add_vals(left, right);
        }
        left
    }

    fn parse_unary(&mut self) -> Val {
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Val::Str(s)
            }
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Val::Num(n)
            }
            Some(Tok::P(p)) => match p.as_str() {
                "-" => {
                    self.pos += 1;
                    match self.parse_unary() {
                        Val::Num(n) => Val::Num(-n),
                        _ => Val::Other("negation".into()),
                    }
                }
                "+" => {
                    self.pos += 1;
                    self.parse_unary()
                }
                "!" | "~" => {
                    self.pos += 1;
                    self.parse_unary();
                    Val::Other("negation".into())
                }
                "++" | "--" => {
                    self.pos += 1;
                    self.parse_unary();
                    Val::Other("increment".into())
                }
                "(" => {
                    self.pos += 1;
                    let v = self.parse_expr();
                    // tolerate junk before the closing paren
                    let mut guard = 0;
                    while !self.eof() && !self.is_p(")") && guard < 10_000 {
                        self.skip_one_balanced();
                        guard += 1;
                    }
                    self.eat_p(")");
                    v
                }
                "[" => self.parse_array(),
                "{" => {
                    let t = self.skip_group();
                    Val::Other(format!("object {t}"))
                }
                _ => Val::Other("unexpected".into()),
            },
            Some(Tok::Ident(id)) => match id.as_str() {
                "true" => {
                    self.pos += 1;
                    Val::Bool(true)
                }
                "false" => {
                    self.pos += 1;
                    Val::Bool(false)
                }
                "null" | "undefined" => {
                    self.pos += 1;
                    Val::Null
                }
                "function" => {
                    self.pos += 1;
                    if matches!(self.peek(), Some(Tok::Ident(_))) {
                        self.pos += 1;
                    }
                    self.skip_function_rest(None);
                    Val::Func
                }
                "typeof" | "void" | "delete" => {
                    self.pos += 1;
                    self.parse_unary();
                    Val::Other("typeof".into())
                }
                "new" => {
                    self.pos += 1;
                    let path = self.parse_path();
                    if self.is_p("(") {
                        let args = self.parse_args();
                        self.emit(format!("new {path}"), EventKind::Call(args));
                    }
                    Val::Other(format!("new {path}"))
                }
                _ => self.parse_ref_or_call(),
            },
            None => Val::Other("end".into()),
        }
    }

    fn parse_array(&mut self) -> Val {
        // current token is '['
        self.pos += 1;
        let mut items = Vec::new();
        let mut guard = 0;
        while !self.eof() && !self.is_p("]") && guard < 100_000 {
            guard += 1;
            if self.eat_p(",") {
                continue;
            }
            let before = self.pos;
            let v = self.parse_expr();
            items.push(v);
            if self.pos == before {
                self.pos += 1;
            }
            // junk between items: skip to the next comma or the closing bracket
            while !self.eof() && !self.is_p(",") && !self.is_p("]") {
                if self.is_p(";") || self.is_p("}") {
                    // unbalanced input; give up on the array
                    return Val::Arr(items);
                }
                self.skip_one_balanced();
            }
        }
        self.eat_p("]");
        Val::Arr(items)
    }

    /// Parse `a.b.c` (dots, and `["x"]` indexes folded into the name). Does not consume a call.
    fn parse_path(&mut self) -> String {
        let mut path = String::new();
        if let Some(Tok::Ident(first)) = self.peek().cloned() {
            self.pos += 1;
            path.push_str(&first);
        }
        loop {
            if self.is_p(".") && matches!(self.peek_at(1), Some(Tok::Ident(_))) {
                self.pos += 1;
                if let Some(Tok::Ident(part)) = self.peek().cloned() {
                    self.pos += 1;
                    path.push('.');
                    path.push_str(&part);
                }
            } else if self.is_p("[") && !path.is_empty() {
                // index: `Context.Options["Fix"]` is kept as `Context.Options[Fix]`
                let inner = self.skip_group();
                let inner = inner.trim_start_matches('[').to_string();
                path.push_str(&format!("[{}]", inner.trim_matches('"')));
            } else {
                break;
            }
        }
        path
    }

    fn parse_ref_or_call(&mut self) -> Val {
        let path = self.parse_path();
        if path.is_empty() {
            self.pos += 1;
            return Val::Other("unexpected".into());
        }
        if self.is_p("(") {
            let args = self.parse_args();
            self.emit(path.clone(), EventKind::Call(args));
            // member access on a call result, `foo().bar`
            while self.is_p(".") && matches!(self.peek_at(1), Some(Tok::Ident(_))) {
                self.pos += 2;
                if self.is_p("(") {
                    self.skip_group();
                }
            }
            return Val::Other(format!("{path}()"));
        }
        if self.is_p("++") || self.is_p("--") {
            self.pos += 1;
            return Val::Other("increment".into());
        }
        if !path.contains('.') && !path.contains('[') {
            if let Some(v) = self.vars.get(&path) {
                return v.clone();
            }
        }
        Val::Ref(path)
    }

    /// Parse `( expr, expr, ... )`; tolerant of junk and unbalanced input.
    fn parse_args(&mut self) -> Vec<Val> {
        let mut args = Vec::new();
        if !self.eat_p("(") {
            return args;
        }
        let mut guard = 0;
        while !self.eof() && !self.is_p(")") && guard < 100_000 {
            guard += 1;
            if self.eat_p(",") {
                continue;
            }
            let before = self.pos;
            let v = self.parse_expr();
            args.push(v);
            if self.pos == before {
                self.pos += 1;
            }
            while !self.eof() && !self.is_p(",") && !self.is_p(")") {
                if self.is_p(";") || self.is_p("}") {
                    return args;
                }
                self.skip_one_balanced();
            }
        }
        self.eat_p(")");
        args
    }

    /// Skip one token, or a whole balanced group when it opens one.
    fn skip_one_balanced(&mut self) {
        if matches!(self.peek(), Some(Tok::P(p)) if p == "(" || p == "[" || p == "{") {
            self.skip_group();
        } else {
            self.pos += 1;
        }
    }

    /// After `function` [name]: skip params, then walk the body as statements one level deeper.
    fn skip_function_rest(&mut self, name: Option<String>) {
        if self.is_p("(") {
            self.skip_group();
        }
        if self.is_p("{") {
            self.pos += 1;
            let saved_func = self.func.take();
            let saved_depth = self.depth;
            // variables declared inside the function go out of scope with it
            let saved_vars = self.vars.clone();
            self.func = name.or(saved_func.clone().map(|f| format!("{f}/anon")));
            // an anonymous nested function keeps events out of the parent's tallies
            self.depth = 1;
            self.parse_block();
            self.func = saved_func;
            self.depth = saved_depth;
            self.vars = saved_vars;
        }
    }

    // ----- statements -----

    /// Parse statements until the matching `}` (consumed) or end of input.
    fn parse_block(&mut self) {
        if self.nest > MAX_NEST {
            // too deep: skip the rest of this block without interpreting it
            let mut level = 1usize;
            while let Some(t) = self.peek() {
                match t {
                    Tok::P(p) if p == "{" => level += 1,
                    Tok::P(p) if p == "}" => {
                        level -= 1;
                        if level == 0 {
                            self.pos += 1;
                            return;
                        }
                    }
                    _ => {}
                }
                self.pos += 1;
            }
            return;
        }
        self.nest += 1;
        while !self.eof() {
            if self.is_p("}") {
                self.pos += 1;
                break;
            }
            let before = self.pos;
            self.parse_statement();
            if self.pos == before {
                self.pos += 1;
            }
        }
        self.nest -= 1;
    }

    fn parse_body_or_statement(&mut self) {
        self.depth += 1;
        if self.is_p("{") {
            self.pos += 1;
            self.parse_block();
        } else {
            self.parse_statement();
        }
        self.depth -= 1;
    }

    fn parse_statement(&mut self) {
        let Some(tok) = self.peek().cloned() else { return };
        match tok {
            Tok::P(p) if p == ";" => {
                self.pos += 1;
            }
            Tok::P(p) if p == "{" => {
                self.pos += 1;
                self.depth += 1;
                self.parse_block();
                self.depth -= 1;
            }
            Tok::Ident(id) => match id.as_str() {
                "var" | "let" | "const" => {
                    self.pos += 1;
                    self.parse_var_decls();
                }
                "function" => {
                    self.pos += 1;
                    let name = match self.peek().cloned() {
                        Some(Tok::Ident(n)) => {
                            self.pos += 1;
                            Some(n)
                        }
                        _ => None,
                    };
                    if let Some(n) = &name {
                        self.emit(n.clone(), EventKind::Assign(Val::Func));
                    }
                    self.skip_function_rest(name);
                }
                "if" | "while" | "switch" | "with" => {
                    self.pos += 1;
                    self.parse_condition();
                    self.parse_body_or_statement();
                    while self.is_ident("else") {
                        self.pos += 1;
                        if self.is_ident("if") {
                            self.pos += 1;
                            self.parse_condition();
                        }
                        self.parse_body_or_statement();
                    }
                }
                "for" => {
                    self.pos += 1;
                    if self.is_p("(") {
                        self.skip_group();
                    }
                    self.parse_body_or_statement();
                }
                "do" => {
                    self.pos += 1;
                    self.parse_body_or_statement();
                    if self.is_ident("while") {
                        self.pos += 1;
                        self.parse_condition();
                    }
                }
                "try" | "finally" => {
                    self.pos += 1;
                    self.parse_body_or_statement();
                }
                "catch" => {
                    self.pos += 1;
                    if self.is_p("(") {
                        self.skip_group();
                    }
                    self.parse_body_or_statement();
                }
                "return" | "throw" => {
                    self.pos += 1;
                    if !self.is_p(";") && !self.is_p("}") {
                        self.parse_expr();
                    }
                    self.eat_p(";");
                }
                "break" | "continue" => {
                    self.pos += 1;
                    self.eat_p(";");
                }
                "else" => {
                    // stray else (broken input): treat as a body
                    self.pos += 1;
                    self.parse_body_or_statement();
                }
                _ => self.parse_expression_statement(),
            },
            _ => {
                // a literal or operator at statement start: parse and discard it
                self.parse_expr();
                self.eat_p(";");
            }
        }
    }

    fn parse_condition(&mut self) {
        if !self.eat_p("(") {
            return;
        }
        self.parse_expr();
        // skip whatever is left up to the matching ')'; stop at '{' or ';' on broken input
        let mut level = 1usize;
        while let Some(t) = self.peek() {
            match t {
                Tok::P(p) if p == "(" => level += 1,
                Tok::P(p) if p == ")" => {
                    level -= 1;
                    if level == 0 {
                        self.pos += 1;
                        return;
                    }
                }
                Tok::P(p) if (p == "{" || p == ";") && level == 1 => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn parse_var_decls(&mut self) {
        loop {
            let Some(Tok::Ident(name)) = self.peek().cloned() else { break };
            self.pos += 1;
            if self.eat_p("=") {
                let v = self.parse_expr();
                self.vars.insert(name, v);
            }
            if !self.eat_p(",") {
                break;
            }
        }
        self.eat_p(";");
    }

    fn parse_expression_statement(&mut self) {
        let start = self.pos;
        let path = self.parse_path();
        if path.is_empty() {
            self.pos = start + 1;
            return;
        }
        // assignment forms: path = , path += , path -=
        if self.is_p("=") || self.is_p("+=") || self.is_p("-=") {
            let append = self.is_p("+=");
            let minus = self.is_p("-=");
            self.pos += 1;
            // chained `a = b = value`: evaluate the right side only
            if self.is_ident("function") {
                self.pos += 1;
                if matches!(self.peek(), Some(Tok::Ident(_))) {
                    self.pos += 1;
                }
                self.emit(path.clone(), EventKind::Assign(Val::Func));
                self.skip_function_rest(Some(path));
                self.eat_p(";");
                return;
            }
            let v = self.parse_expr();
            if !path.contains('.') && !path.contains('[') && !minus {
                // plain variable assignment keeps `vars` current for later references
                let new = match (append, self.vars.get(&path)) {
                    (true, Some(prev)) => add_vals(prev.clone(), v.clone()),
                    _ => v.clone(),
                };
                self.vars.insert(path.clone(), new);
            }
            let kind = if minus {
                EventKind::Assign(Val::Other("subtraction".into()))
            } else if append {
                EventKind::AddAssign(v)
            } else {
                EventKind::Assign(v)
            };
            self.emit(path, kind);
            self.eat_p(";");
            return;
        }
        if self.is_p("(") {
            let args = self.parse_args();
            self.emit(path, EventKind::Call(args));
            // trailing member calls such as `.toString()`
            while self.is_p(".") && matches!(self.peek_at(1), Some(Tok::Ident(_))) {
                self.pos += 2;
                if self.is_p("(") {
                    self.skip_group();
                }
            }
            self.eat_p(";");
            return;
        }
        if self.is_p("++") || self.is_p("--") {
            self.pos += 1;
            self.eat_p(";");
            return;
        }
        // a bare expression (or something we do not understand): skip to the end of the statement
        let mut guard = 0;
        while !self.eof() && !self.is_p(";") && !self.is_p("}") && guard < 10_000 {
            guard += 1;
            // a new line starting with `Game.`/`Hub.` after a missing semicolon ends it too
            if let Some(Tok::Ident(n)) = self.peek() {
                if (n == "Game" || n == "Hub") && guard > 1 {
                    break;
                }
            }
            self.skip_one_balanced();
        }
        self.eat_p(";");
    }
}

/// `a + b` for the supported literal shapes; otherwise a `Concat` of the pieces.
pub(crate) fn add_vals(a: Val, b: Val) -> Val {
    if let (Val::Num(x), Val::Num(y)) = (&a, &b) {
        return Val::Num(x + y);
    }
    let stringy = |v: &Val| matches!(v, Val::Str(_) | Val::Concat(_));
    let other = |v: &Val| matches!(v, Val::Other(_));
    if !stringy(&a) && !stringy(&b) && (other(&a) || other(&b)) {
        return Val::Other("expression".into());
    }
    let mut parts: Vec<Val> = Vec::new();
    for v in [a, b] {
        match v {
            Val::Concat(inner) => parts.extend(inner),
            Val::Num(n) => parts.push(Val::Str(num_to_string(n))),
            Val::Bool(x) => parts.push(Val::Str(x.to_string())),
            Val::Null => parts.push(Val::Str("null".into())),
            other => parts.push(other),
        }
    }
    // merge adjacent string literals
    let mut merged: Vec<Val> = Vec::new();
    for p in parts {
        if let (Some(Val::Str(prev)), Val::Str(s)) = (merged.last_mut(), &p) {
            prev.push_str(s);
        } else {
            merged.push(p);
        }
    }
    if merged.len() == 1 { merged.pop().unwrap() } else { Val::Concat(merged) }
}

/// Scan a handler script. Never fails.
pub fn parse_handler(script: &str) -> ParsedHandler {
    let mut p = Parser {
        toks: lex(script),
        pos: 0,
        vars: BTreeMap::new(),
        events: Vec::new(),
        func: None,
        depth: 0,
        nest: 0,
    };
    let mut guard = 0usize;
    while !p.eof() && guard < 5_000_000 {
        guard += 1;
        let before = p.pos;
        p.parse_statement();
        if p.pos == before {
            p.pos += 1;
        }
    }
    build(p.events)
}

fn strip_game(path: &str) -> Option<&str> {
    path.strip_prefix("Game.")
}

fn build(events: Vec<Event>) -> ParsedHandler {
    let mut out = ParsedHandler::default();
    let mut play_overrides: Vec<(String, Val)> = Vec::new();

    for ev in &events {
        match (&ev.func, ev.depth) {
            (None, 0) => match &ev.kind {
                EventKind::Assign(v) => {
                    if matches!(v, Val::Func) {
                        out.functions.insert(ev.path.clone());
                    }
                    if let Some(key) = strip_game(&ev.path) {
                        out.fields.insert(key.to_string(), v.clone());
                    } else if let Some(key) = ev.path.strip_prefix("Hub.") {
                        out.hub.insert(key.to_string(), v.clone());
                    }
                }
                EventKind::AddAssign(v) => {
                    // `Game.X += "..."` at top level: extend a string/array when possible
                    if let Some(key) = strip_game(&ev.path) {
                        if let Some(prev) = out.fields.get_mut(key) {
                            *prev = add_vals(prev.clone(), v.clone());
                        }
                    }
                }
                EventKind::Call(args) => {
                    if ev.path == "Game.AddOption" {
                        let s = |i: usize| args.get(i).map(Val::display).unwrap_or_default();
                        let values = args.get(3).and_then(Val::as_str_list).unwrap_or_default();
                        out.options.push(UserOption {
                            title: s(0),
                            description: s(1),
                            key: s(2),
                            values,
                        });
                    }
                }
            },
            (Some(f), d) if f == "Game.Play" => {
                if d >= 1 {
                    out.play_statements += 1;
                }
                match &ev.kind {
                    EventKind::Assign(v) | EventKind::AddAssign(v) => {
                        let append = matches!(ev.kind, EventKind::AddAssign(_));
                        if let Some(key) = strip_game(&ev.path) {
                            if matches!(v, Val::Func) {
                                out.functions.insert(ev.path.clone());
                            }
                            if d == 1 && !append {
                                play_overrides.push((key.to_string(), v.clone()));
                            } else {
                                out.conditional.entry(key.to_string()).or_default().push(v.clone());
                            }
                        } else if let Some(name) = ev.path.strip_prefix("Context.") {
                            out.play_context_sets.push((name.to_string(), append, v.clone(), d));
                        }
                    }
                    EventKind::Call(args) => {
                        out.play_calls.push((ev.path.clone(), args.clone(), d));
                    }
                }
            }
            (Some(_), _) => {
                // inside some other function (OnInputLocked, ...): only record that it exists
                if let EventKind::Assign(Val::Func) = &ev.kind {
                    out.functions.insert(ev.path.clone());
                }
            }
            (None, _) => {
                // top-level control flow (`if (...) { Game.X = ... }`): conditional assignments
                if let (EventKind::Assign(v), Some(key)) = (&ev.kind, strip_game(&ev.path)) {
                    out.conditional.entry(key.to_string()).or_default().push(v.clone());
                }
            }
        }
    }
    for (k, v) in play_overrides {
        out.fields.insert(k, v);
    }
    out
}

impl ParsedHandler {
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.fields.get(key)
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.fields.get(key).and_then(Val::as_bool)
    }

    pub fn text(&self, key: &str) -> Option<String> {
        match self.fields.get(key)? {
            Val::Str(s) => Some(s.clone()),
            Val::Num(n) => Some(num_to_string(*n)),
            _ => None,
        }
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.fields.get(key).and_then(Val::as_f64)
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        self.fields.get(key).and_then(Val::as_str_list).unwrap_or_default()
    }

    pub fn has_play(&self) -> bool {
        self.functions.contains("Game.Play")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_comments_strings_and_hex() {
        let t = lex("a = 0x23; // note\n/* x */ b = 'it\\'s';");
        assert!(t.contains(&Tok::Num(35.0)));
        assert!(t.contains(&Tok::Str("it's".into())));
    }

    #[test]
    fn concat_merges_literals_and_keeps_refs() {
        let p = parse_handler("Game.StartArguments = '-w ' + \"x\" + Context.Width + ' -h ' + Context.Height;");
        match p.get("StartArguments").unwrap() {
            Val::Concat(parts) => {
                assert_eq!(parts[0], Val::Str("-w x".into()));
                assert_eq!(parts[1], Val::Ref("Context.Width".into()));
            }
            other => panic!("{other:?}"),
        }
    }
}
