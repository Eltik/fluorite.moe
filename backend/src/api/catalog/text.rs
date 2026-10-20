//! Localized text: i18n id resolution with fallback, `{F}/{M}` gender variants,
//! `{key:fmt}` blackboard placeholders, and `<@style>…</>` / `<#term>…</>` markup turned
//! into renderer-friendly [`Segment`]s.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

use crate::api::i18n::TextMap;

/// An i18n reference as stored in tables: `{ "id": <int64>, "text": "" }`. `id == 0` is
/// "no text". Ids are kept as `i64` internally and only ever leave the server as strings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct TextRef {
    #[serde(default, deserialize_with = "de_i64")]
    pub id: i64,
}

impl TextRef {
    pub fn is_empty(self) -> bool {
        self.id == 0
    }

    /// The i18n table key (the signed decimal id).
    pub fn key(self) -> String {
        self.id.to_string()
    }
}

/// Accept an int64 id as a JSON number (signed or unsigned two's-complement) or a string.
pub fn de_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        I(i64),
        U(u64),
        F(f64),
        S(String),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::I(i) => i,
        Raw::U(u) => u as i64,
        Raw::F(f) => f as i64,
        Raw::S(s) => s.trim().parse().map_err(serde::de::Error::custom)?,
    })
}

/// Endministrator gender used to pick `{F}…{M}…` variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Gender {
    #[default]
    Female,
    Male,
}

impl Gender {
    pub fn parse(s: Option<&str>) -> Result<Gender, String> {
        match s.map(|s| s.to_ascii_lowercase()) {
            None => Ok(Gender::Female),
            Some(s) if s.is_empty() || s == "f" || s == "female" => Ok(Gender::Female),
            Some(s) if s == "m" || s == "male" => Ok(Gender::Male),
            Some(s) => Err(format!("invalid gender {s:?} (use f or m)")),
        }
    }

    pub fn other(self) -> Gender {
        match self {
            Gender::Female => Gender::Male,
            Gender::Male => Gender::Female,
        }
    }
}

/// Whether `s` carries `{F}…{M}…` variants.
pub fn has_gender_variants(s: &str) -> bool {
    s.contains("{F}") && s.contains("{M}")
}

/// Pick one gender's variant from a `{F}female{M}male` string (either order); strings
/// without both markers come back unchanged.
pub fn pick_gender(s: &str, g: Gender) -> &str {
    let (Some(f), Some(m)) = (s.find("{F}"), s.find("{M}")) else {
        return s;
    };
    let (start, end) = match (g, f < m) {
        (Gender::Female, true) => (f + 3, m),
        (Gender::Female, false) => (f + 3, s.len()),
        (Gender::Male, true) => (m + 3, s.len()),
        (Gender::Male, false) => (m + 3, f),
    };
    &s[start..end]
}

// ── Blackboard placeholders ──────────────────────────────────────────────

/// One `{key, value, valueStr}` blackboard row.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BbEntry {
    pub key: String,
    #[serde(default)]
    pub value: f64,
    #[serde(default, rename = "valueStr")]
    pub value_str: String,
}

/// Values available to `{key:fmt}` placeholders.
#[derive(Debug, Clone, Default)]
pub struct Blackboard {
    pub nums: HashMap<String, f64>,
    pub strs: HashMap<String, String>,
}

impl Blackboard {
    /// Add rows; keys already present keep their first value.
    pub fn extend<'a>(&mut self, rows: impl IntoIterator<Item = &'a BbEntry>) {
        for r in rows {
            self.nums.entry(r.key.clone()).or_insert(r.value);
            if !r.value_str.is_empty() {
                self.strs
                    .entry(r.key.clone())
                    .or_insert_with(|| r.value_str.clone());
            }
        }
    }

    pub fn from_rows<'a>(rows: impl IntoIterator<Item = &'a BbEntry>) -> Self {
        let mut b = Self::default();
        b.extend(rows);
        b
    }

    pub fn is_empty(&self) -> bool {
        self.nums.is_empty() && self.strs.is_empty()
    }
}

/// Evaluate a placeholder expression (`atk_up`, `1-costvalue`, `spell_dmg_up2*max_stack`,
/// `-combo_cd_enhance`, parentheses) against `bb`. `None` when malformed or a name is unknown.
pub fn eval_expr(expr: &str, bb: &Blackboard) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
        bb: &'a Blackboard,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i] == b' ' {
                self.i += 1;
            }
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'+') => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    Some(b'-') => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.factor()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'*') => {
                        self.i += 1;
                        v *= self.factor()?;
                    }
                    Some(b'/') => {
                        self.i += 1;
                        let d = self.factor()?;
                        if d == 0.0 {
                            return None;
                        }
                        v /= d;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn factor(&mut self) -> Option<f64> {
            self.ws();
            let c = *self.s.get(self.i)?;
            if c == b'-' {
                self.i += 1;
                return Some(-self.factor()?);
            }
            if c == b'(' {
                self.i += 1;
                let v = self.expr()?;
                self.ws();
                if self.s.get(self.i) != Some(&b')') {
                    return None;
                }
                self.i += 1;
                return Some(v);
            }
            let start = self.i;
            while self.i < self.s.len()
                && (self.s[self.i].is_ascii_alphanumeric()
                    || self.s[self.i] == b'_'
                    || self.s[self.i] == b'.')
            {
                self.i += 1;
            }
            if start == self.i {
                return None;
            }
            let tok = std::str::from_utf8(&self.s[start..self.i]).ok()?;
            if tok.as_bytes()[0].is_ascii_digit() {
                // A number, unless it is an identifier that happens to start with a digit.
                if let Ok(n) = tok.parse::<f64>() {
                    return Some(n);
                }
            }
            self.bb.nums.get(tok).copied()
        }
    }
    let mut p = P {
        s: expr.trim().as_bytes(),
        i: 0,
        bb,
    };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

/// Format like the game's C# numeric format strings: `0`, `0.0`, `0.00`, `0.##`, and the
/// same with a trailing `%` (value × 100). An empty format prints the shortest form.
/// Rounds half away from zero (with a tiny epsilon to absorb `f32` storage error).
pub fn format_number(v: f64, fmt: &str) -> String {
    let fmt = fmt.trim();
    let (mut v, percent, body) = match fmt.strip_suffix('%') {
        Some(b) => (v * 100.0, true, b),
        None => (v, false, fmt),
    };
    let numeric = !body.is_empty() && body.bytes().all(|b| matches!(b, b'0' | b'#' | b'.' | b','));
    let (min, max) = if numeric {
        match body.split_once('.') {
            Some((_, frac)) => (
                frac.bytes().filter(|&b| b == b'0').count(),
                frac.bytes().filter(|&b| b == b'0' || b == b'#').count(),
            ),
            None => (0, 0),
        }
    } else {
        // General format: up to 4 decimals, trailing zeros trimmed.
        (0, 4)
    };
    let scale = 10f64.powi(max as i32);
    v = (v * scale + v.signum() * 1e-6).round() / scale;
    if v == 0.0 {
        v = 0.0; // no "-0"
    }
    let mut s = format!("{v:.max$}");
    if max > min && s.contains('.') {
        let keep = s.find('.').unwrap() + 1 + min;
        while s.len() > keep && s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if percent {
        s.push('%');
    }
    s
}

/// Substitute every `{expr}` / `{expr:fmt}` placeholder that evaluates against `bb`;
/// anything else (gender markers, unknown names, non-placeholder braces) is left as-is.
pub fn render_placeholders(s: &str, bb: &Blackboard) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after
            .find(['}', '{'])
            .filter(|&i| after.as_bytes()[i] == b'}')
        else {
            out.push('{');
            rest = after;
            continue;
        };
        let body = &after[..close];
        let (expr, fmt) = body.split_once(':').unwrap_or((body, ""));
        let replaced = if fmt.is_empty() && bb.strs.contains_key(expr.trim()) {
            bb.strs.get(expr.trim()).cloned()
        } else {
            eval_expr(expr, bb).map(|v| format_number(v, fmt))
        };
        match replaced {
            Some(r) => out.push_str(&r),
            None => {
                out.push('{');
                out.push_str(body);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

// ── Markup ───────────────────────────────────────────────────────────────

/// A run of text with uniform formatting. Empty-text segments carry an inline `image`
/// (sprite name) or `action` (input-key glyph).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub text: String,
    /// Rich-text style id (`ba.vup`), from `<@ba.vup>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Resolved display color (`#rrggbb`) from the style/term or an explicit `<color>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Glossary term id (`ba.noguard`), from `<#ba.noguard>`; see the response's `terms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

pub type Rich = Vec<Segment>;

/// Style colors and term definitions used while parsing markup.
#[derive(Debug, Clone, Default)]
pub struct Markup {
    /// Style id → display color.
    pub styles: HashMap<String, String>,
    pub terms: HashMap<String, TermDef>,
}

#[derive(Debug, Clone, Default)]
pub struct TermDef {
    pub name: TextRef,
    pub desc: TextRef,
    pub icon: Option<String>,
    /// Style id giving the term's color.
    pub style: Option<String>,
}

impl Markup {
    fn term_color(&self, id: &str) -> Option<String> {
        let style = self.terms.get(id)?.style.as_deref()?;
        self.styles.get(style).cloned()
    }
}

/// First `#rrggbb[aa]` color in a `<color=…>` definition list.
pub fn color_of(defs: &[String]) -> Option<String> {
    defs.iter().find_map(|d| {
        let v = d.strip_prefix("<color=")?.strip_suffix('>')?;
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        v.starts_with('#').then(|| v.to_string())
    })
}

#[derive(Debug, Clone)]
enum Frame {
    Style(String),
    Term(String),
    Color(String),
    Bold,
    Italic,
    Ignored(&'static str),
}

fn tag_value(inner: &str) -> String {
    let v = inner.split_once('=').map_or("", |(_, v)| v).trim();
    if let Some(q) = v.strip_prefix('"') {
        q.split('"').next().unwrap_or("").to_string()
    } else {
        v.split_whitespace().next().unwrap_or("").to_string()
    }
}

/// Parse game rich text into segments. Unknown tags are kept as literal text.
pub fn parse_markup(s: &str, m: &Markup) -> Rich {
    let mut out: Rich = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut buf = String::new();
    let mut in_image = false;

    let current = |stack: &[Frame]| -> Segment {
        let mut seg = Segment::default();
        for f in stack {
            match f {
                Frame::Style(id) => {
                    seg.style = Some(id.clone());
                    if let Some(c) = m.styles.get(id) {
                        seg.color = Some(c.clone());
                    }
                }
                Frame::Term(id) => {
                    seg.term = Some(id.clone());
                    if let Some(c) = m.term_color(id) {
                        seg.color = Some(c);
                    }
                }
                Frame::Color(c) => seg.color = Some(c.clone()),
                Frame::Bold => seg.bold = true,
                Frame::Italic => seg.italic = true,
                Frame::Ignored(_) => {}
            }
        }
        seg
    };
    let flush = |out: &mut Rich, buf: &mut String, stack: &[Frame]| {
        if buf.is_empty() {
            return;
        }
        let mut seg = current(stack);
        seg.text = std::mem::take(buf);
        push_segment(out, seg);
    };

    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        buf.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        let close = after
            .find(['>', '<'])
            .filter(|&i| after.as_bytes()[i] == b'>' && i <= 256);
        let Some(close) = close else {
            buf.push('<');
            rest = after;
            continue;
        };
        let inner = &after[..close];
        let lower = inner.to_ascii_lowercase();
        let mut handled = true;
        if in_image {
            if lower == "/image" {
                let name = std::mem::take(&mut buf);
                push_segment(
                    &mut out,
                    Segment {
                        image: Some(name.trim().to_string()),
                        ..current(&stack)
                    },
                );
                in_image = false;
            } else {
                handled = false;
            }
        } else if inner == "/" {
            flush(&mut out, &mut buf, &stack);
            if let Some(i) = stack
                .iter()
                .rposition(|f| matches!(f, Frame::Style(_) | Frame::Term(_)))
            {
                stack.remove(i);
            }
        } else if let Some(id) = inner.strip_prefix('@').filter(|id| !id.is_empty()) {
            flush(&mut out, &mut buf, &stack);
            stack.push(Frame::Style(id.to_string()));
        } else if let Some(id) = inner
            .strip_prefix('#')
            .filter(|id| !id.is_empty() && !id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            flush(&mut out, &mut buf, &stack);
            stack.push(Frame::Term(id.to_string()));
        } else if lower.starts_with("color=") || lower.starts_with('#') {
            flush(&mut out, &mut buf, &stack);
            let v = if lower.starts_with('#') {
                inner.to_string()
            } else {
                tag_value(inner)
            };
            stack.push(Frame::Color(v));
        } else if lower == "b" || lower == "i" {
            flush(&mut out, &mut buf, &stack);
            stack.push(if lower == "b" {
                Frame::Bold
            } else {
                Frame::Italic
            });
        } else if lower.starts_with("size=") || lower.starts_with("link=") || lower == "u" {
            flush(&mut out, &mut buf, &stack);
            let tag = if lower == "u" {
                "u"
            } else if lower.starts_with("size") {
                "size"
            } else {
                "link"
            };
            stack.push(Frame::Ignored(tag));
        } else if let Some(name) = lower.strip_prefix('/') {
            let pos = match name {
                "color" => stack.iter().rposition(|f| matches!(f, Frame::Color(_))),
                "b" => stack.iter().rposition(|f| matches!(f, Frame::Bold)),
                "i" => stack.iter().rposition(|f| matches!(f, Frame::Italic)),
                "size" | "link" | "u" => stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::Ignored(t) if *t == name)),
                _ => None,
            };
            match pos {
                Some(i) => {
                    flush(&mut out, &mut buf, &stack);
                    stack.remove(i);
                }
                None => handled = false,
            }
        } else if lower == "image" {
            flush(&mut out, &mut buf, &stack);
            in_image = true;
        } else if lower.starts_with("image=") || lower.starts_with("sprite") {
            flush(&mut out, &mut buf, &stack);
            push_segment(
                &mut out,
                Segment {
                    image: Some(tag_value(inner)),
                    ..current(&stack)
                },
            );
        } else if lower.starts_with("action=") {
            flush(&mut out, &mut buf, &stack);
            push_segment(
                &mut out,
                Segment {
                    action: Some(tag_value(inner)),
                    ..current(&stack)
                },
            );
        } else {
            handled = false;
        }
        if !handled {
            buf.push('<');
            buf.push_str(inner);
            buf.push('>');
        }
        rest = &after[close + 1..];
    }
    buf.push_str(rest);
    flush(&mut out, &mut buf, &stack);
    out
}

/// Append, merging with the previous text segment when the formatting is identical.
fn push_segment(out: &mut Rich, seg: Segment) {
    if seg.image.is_none()
        && seg.action.is_none()
        && let Some(last) = out.last_mut()
        && last.image.is_none()
        && last.action.is_none()
        && last.style == seg.style
        && last.color == seg.color
        && last.term == seg.term
        && last.bold == seg.bold
        && last.italic == seg.italic
    {
        last.text.push_str(&seg.text);
        return;
    }
    out.push(seg);
}

/// Plain text of a markup string (tags removed, images/actions dropped).
pub fn strip_markup(s: &str) -> String {
    if !s.contains('<') {
        return s.to_string();
    }
    parse_markup(s, &Markup::default())
        .into_iter()
        .map(|seg| seg.text)
        .collect()
}

// ── Resolution ───────────────────────────────────────────────────────────

/// Localized text resolution for one request: the requested language with EN then CN
/// fallback for missing/empty strings, a gender choice, and the markup tables. Records
/// which glossary terms were emitted so responses can ship just those definitions.
pub struct Texts {
    pub lang: String,
    pub gender: Gender,
    pub primary: Arc<TextMap>,
    pub fallbacks: Vec<Arc<TextMap>>,
    pub markup: Arc<Markup>,
    used_terms: RefCell<BTreeSet<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermOut {
    pub name: String,
    pub description: Rich,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Texts {
    pub fn new(
        lang: String,
        gender: Gender,
        primary: Arc<TextMap>,
        fallbacks: Vec<Arc<TextMap>>,
        markup: Arc<Markup>,
    ) -> Self {
        Self {
            lang,
            gender,
            primary,
            fallbacks,
            markup,
            used_terms: RefCell::new(BTreeSet::new()),
        }
    }

    /// Raw localized string (markup and variants intact), `None` when absent everywhere.
    pub fn raw(&self, r: TextRef) -> Option<&str> {
        if r.is_empty() {
            return None;
        }
        self.raw_key(&r.key())
    }

    pub fn raw_key(&self, key: &str) -> Option<&str> {
        std::iter::once(&self.primary)
            .chain(self.fallbacks.iter())
            .find_map(|m| m.get(key).map(String::as_str).filter(|s| !s.is_empty()))
    }

    /// Display string: gender variant picked, markup stripped. `""` when absent.
    pub fn plain(&self, r: TextRef) -> String {
        self.raw(r)
            .map(|s| strip_markup(pick_gender(s, self.gender)))
            .unwrap_or_default()
    }

    pub fn plain_opt(&self, r: TextRef) -> Option<String> {
        Some(self.plain(r)).filter(|s| !s.is_empty())
    }

    /// Rich text with placeholders filled from `bb`.
    pub fn rich(&self, r: TextRef, bb: Option<&Blackboard>) -> Rich {
        match self.raw(r) {
            Some(s) => self.rich_str(s, bb, self.gender),
            None => Vec::new(),
        }
    }

    pub fn rich_str(&self, s: &str, bb: Option<&Blackboard>, gender: Gender) -> Rich {
        let s = pick_gender(s, gender);
        let rendered;
        let s = match bb {
            Some(bb) if !bb.is_empty() => {
                rendered = render_placeholders(s, bb);
                rendered.as_str()
            }
            _ => s,
        };
        let segs = parse_markup(s, &self.markup);
        let mut used = self.used_terms.borrow_mut();
        for seg in &segs {
            if let Some(t) = &seg.term {
                used.insert(t.clone());
            }
        }
        segs
    }

    /// Definitions of every glossary term emitted so far (descriptions are not expanded
    /// further).
    pub fn terms(&self) -> HashMap<String, TermOut> {
        let used: Vec<String> = self.used_terms.borrow().iter().cloned().collect();
        used.into_iter()
            .filter_map(|id| {
                let def = self.markup.terms.get(&id)?;
                let desc = self
                    .raw(def.desc)
                    .map(|s| parse_markup(pick_gender(s, self.gender), &self.markup))
                    .unwrap_or_default();
                Some((
                    id.clone(),
                    TermOut {
                        name: self.plain(def.name),
                        description: desc,
                        icon: def.icon.clone(),
                        color: self.markup.term_color(&id),
                    },
                ))
            })
            .collect()
    }
}
