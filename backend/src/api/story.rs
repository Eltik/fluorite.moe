//! `/api/story`: dialog graphs from `gamedata/.../dialog/json/{mission,level,gameplay,npc}`
//! listed by category/group, and one dialog flattened into reading order with localized
//! speakers, lines and options.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use serde::Serialize;
use serde_json::Value;

use super::SharedState;
use super::catalog::tables::{
    ActorImageTable, DialogOptionTable, DialogSummaryTable, DialogTextTable, TextTable,
};
use super::catalog::text::{Gender, Rich, Texts, has_gender_variants};
use super::catalog::{LangQuery, Memo, assets, blocking, dep, tables_dir, text_maps, texts};
use super::error::{ApiError, ApiResult};
use super::paths::{Root, is_hidden};

/// Dialog JSON folder, relative to the gamedata root.
pub const DIALOG_DIR: &str = "beyond/dynamicassets/gameplay/dialog/json";
const CATEGORIES: [&str; 4] = ["mission", "level", "gameplay", "npc"];

#[derive(Debug, Clone)]
pub struct DialogEntry {
    pub id: String,
    pub category: String,
    /// Sub-folder path between the category and the file (`a1m10`, `map01/lv001`, or "").
    pub group: String,
    /// Gamedata-root relative path.
    pub rel: String,
}

#[derive(Debug, Default)]
pub struct StoryIndex {
    pub entries: Vec<DialogEntry>,
    pub by_id: HashMap<String, usize>,
}

/// Natural ordering: digit runs compare numerically (`dlg_2` < `dlg_10`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let db = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let na = std::str::from_utf8(&a[..da])
                    .unwrap_or("0")
                    .trim_start_matches('0');
                let nb = std::str::from_utf8(&b[..db])
                    .unwrap_or("0")
                    .trim_start_matches('0');
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

impl StoryIndex {
    /// Walk the dialog folder (blocking).
    pub fn build(gamedata: &Path) -> Self {
        fn walk(abs: &Path, rel: &str, depth: u32, out: &mut Vec<(String, String)>) {
            let Ok(rd) = std::fs::read_dir(abs) else {
                return;
            };
            for e in rd.flatten() {
                let Ok(name) = e.file_name().into_string() else {
                    continue;
                };
                if is_hidden(&name) {
                    continue;
                }
                let child = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                match e.file_type() {
                    Ok(t) if t.is_dir() && depth < 4 => walk(&e.path(), &child, depth + 1, out),
                    Ok(t) if t.is_file() && name.ends_with(".json") => out.push((name, child)),
                    _ => {}
                }
            }
        }
        let root = gamedata.join(DIALOG_DIR);
        let mut entries = Vec::new();
        for cat in CATEGORIES {
            let mut files = Vec::new();
            walk(&root.join(cat), "", 0, &mut files);
            for (name, rel) in files {
                let id = name.trim_end_matches(".json").to_string();
                let group = rel
                    .rsplit_once('/')
                    .map_or(String::new(), |(g, _)| g.to_string());
                entries.push(DialogEntry {
                    id,
                    category: cat.to_string(),
                    group,
                    rel: format!("{DIALOG_DIR}/{cat}/{rel}"),
                });
            }
        }
        entries.sort_by(|a, b| {
            let ca = CATEGORIES.iter().position(|c| *c == a.category);
            let cb = CATEGORIES.iter().position(|c| *c == b.category);
            ca.cmp(&cb)
                .then_with(|| natural_cmp(&a.group, &b.group))
                .then_with(|| natural_cmp(&a.id, &b.id))
        });
        let mut by_id = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            by_id.entry(e.id.clone()).or_insert(i);
        }
        Self { entries, by_id }
    }
}

pub struct StoryCache {
    index: Memo<(), StoryIndex>,
    lists: Memo<String, StoryList>,
}

impl Default for StoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl StoryCache {
    pub fn new() -> Self {
        Self {
            index: Memo::new(1, Some(Duration::from_secs(120))),
            lists: Memo::new(16, None),
        }
    }
}

async fn story_index(state: &SharedState) -> ApiResult<Arc<StoryIndex>> {
    let cache = &state.catalog.story;
    if let Some(i) = cache.index.get(&(), &[]) {
        return Ok(i);
    }
    let gamedata = Root::Gamedata.dir(&state.config.output);
    let started = Instant::now();
    let ix = Arc::new(blocking(move || StoryIndex::build(&gamedata)).await?);
    tracing::info!(
        "story index: {} dialogs in {} ms",
        ix.entries.len(),
        started.elapsed().as_millis()
    );
    cache.index.put((), Vec::new(), ix.clone());
    Ok(ix)
}

// ── List ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupOut {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub dialogs: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryOut {
    pub id: String,
    pub count: usize,
    pub groups: Vec<GroupOut>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryList {
    pub lang: String,
    pub total: usize,
    pub categories: Vec<CategoryOut>,
}

fn build_list(ix: &StoryIndex, text: &TextTable, texts: &Texts) -> StoryList {
    let mut categories: Vec<CategoryOut> = Vec::new();
    for e in &ix.entries {
        if categories.last().is_none_or(|c| c.id != e.category) {
            categories.push(CategoryOut {
                id: e.category.clone(),
                count: 0,
                groups: Vec::new(),
            });
        }
        let cat = categories.last_mut().expect("pushed above");
        cat.count += 1;
        if cat.groups.last().is_none_or(|g| g.id != e.group) {
            let title = if e.category == "mission" {
                text.get(&format!("{}_name", e.group))
                    .and_then(|r| texts.plain_opt(*r))
            } else {
                None
            };
            cat.groups.push(GroupOut {
                id: e.group.clone(),
                title,
                dialogs: Vec::new(),
            });
        }
        cat.groups
            .last_mut()
            .expect("pushed above")
            .dialogs
            .push(e.id.clone());
    }
    StoryList {
        lang: texts.lang.clone(),
        total: ix.entries.len(),
        categories,
    }
}

/// `GET /api/story?lang=`
pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<LangQuery>,
) -> ApiResult<axum::response::Response> {
    let (lang, gender) = q.parse()?;
    let ix = story_index(&state).await?;
    let text = state
        .catalog
        .tables
        .get_or_default::<TextTable>(&tables_dir(&state))
        .await?;
    let (primary, fallbacks) = text_maps(&state, &lang).await?;
    let mut deps = vec![dep(&ix), dep(&text), dep(&primary)];
    deps.extend(fallbacks.iter().map(dep));
    let cache = &state.catalog.story;
    let out = match cache.lists.get(&lang, &deps) {
        Some(l) => l,
        None => {
            let texts = texts(&state, &lang, gender).await?;
            let ix2 = ix.clone();
            let l = Arc::new(blocking(move || build_list(&ix2, &text, &texts)).await?);
            cache.lists.put(lang, deps, l.clone());
            l
        }
    };
    let bytes = serde_json::to_vec(&*out).map_err(|e| ApiError::internal(e.to_string()))?;
    super::catalog::json_bytes(bytes)
}

// ── Dialog ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Speaker {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionOut {
    pub id: String,
    pub text: Rich,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeOut {
    pub id: String,
    /// `line` | `option` | `transition` | `finish`
    pub kind: &'static str,
    /// For transitions: `transition`, `cinematic`, `if`, `branch`, `dialog`, `summary`,
    /// `empty`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<Speaker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<Rich>,
    /// The other gender's variant, present only when it differs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<Rich>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<OptionOut>>,
    /// Linked dialog / cutscene id for `dialog` and `cinematic` transitions.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogOut {
    pub id: String,
    pub category: String,
    pub group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub lang: String,
    pub gender: &'static str,
    pub start: Option<String>,
    pub nodes: Vec<NodeOut>,
}

/// Lookups a dialog flattening needs.
pub struct DialogTables<'a> {
    pub lines: &'a DialogTextTable,
    pub options: &'a DialogOptionTable,
    pub summaries: &'a DialogSummaryTable,
    /// Speaker code (`pelica`) → avatar texture path.
    pub avatars: &'a HashMap<String, String>,
}

fn short_type(t: &str) -> &str {
    let s = t.rsplit('.').next().unwrap_or(t);
    let s = s
        .strip_prefix("DialogTree")
        .or_else(|| s.strip_prefix("Dialog"))
        .unwrap_or(s);
    s.strip_suffix("Node").unwrap_or(s)
}

/// Performance-only nodes spliced out of the reading order.
fn is_spliced(short: &str) -> bool {
    matches!(short, "ExActor" | "ExCamera" | "ExLight" | "OpenUI")
}

/// First non-empty string under any of `keys`, searched depth-first.
fn find_str<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match v {
        Value::Object(o) => {
            for k in keys {
                if let Some(Value::String(s)) = o.get(*k)
                    && !s.is_empty()
                {
                    return Some(s);
                }
            }
            o.values().find_map(|x| find_str(x, keys))
        }
        Value::Array(a) => a.iter().find_map(|x| find_str(x, keys)),
        _ => None,
    }
}

fn collect_strs(v: &Value, key: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                match x {
                    Value::String(s) if k == key && !s.is_empty() => out.push(s.clone()),
                    _ => collect_strs(x, key, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strs(x, key, out)),
        _ => {}
    }
}

fn node_ref(v: &Value) -> Option<String> {
    let o = v.as_object()?;
    o.get("$ref")
        .or_else(|| o.get("$id"))
        .and_then(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

/// Flatten a NodeCanvas dialog tree into reading order.
pub fn flatten(tree: &Value, t: &DialogTables, texts: &Texts) -> (Option<String>, Vec<NodeOut>) {
    let empty = Vec::new();
    let raw_nodes = tree
        .get("nodes")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let nodes: Vec<(String, &str, &Value)> = raw_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let id = node_ref(n).unwrap_or_else(|| format!("_n{i}"));
            let ty = n.get("$type").and_then(Value::as_str).unwrap_or("");
            (id, short_type(ty), n)
        })
        .collect();
    let by_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _, _))| (id.as_str(), i))
        .collect();

    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for c in tree
        .get("connections")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        let (Some(s), Some(d)) = (
            c.get("_sourceNode").and_then(node_ref),
            c.get("_targetNode").and_then(node_ref),
        ) else {
            continue;
        };
        if by_id.contains_key(s.as_str()) && by_id.contains_key(d.as_str()) {
            edges.entry(s).or_default().push(d);
        }
    }

    let spliced = |id: &str| by_id.get(id).is_some_and(|&i| is_spliced(nodes[i].1));
    // Resolve a target through spliced nodes.
    fn resolve(
        id: &str,
        edges: &HashMap<String, Vec<String>>,
        spliced: &dyn Fn(&str) -> bool,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if !seen.insert(id.to_string()) {
            return;
        }
        if spliced(id) {
            for n in edges.get(id).map(Vec::as_slice).unwrap_or(&[]) {
                resolve(n, edges, spliced, seen, out);
            }
        } else if !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    }
    let targets = |id: &str| -> Vec<Vec<String>> {
        edges
            .get(id)
            .map(|v| {
                v.iter()
                    .map(|n| {
                        let mut out = Vec::new();
                        resolve(n, &edges, &spliced, &mut HashSet::new(), &mut out);
                        out
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let kept: Vec<usize> = (0..nodes.len())
        .filter(|&i| !is_spliced(nodes[i].1))
        .collect();
    let mut eff: HashMap<&str, Vec<String>> = HashMap::new();
    let mut per_edge: HashMap<&str, Vec<Vec<String>>> = HashMap::new();
    for &i in &kept {
        let id = nodes[i].0.as_str();
        let per = targets(id);
        let mut flat: Vec<String> = Vec::new();
        for t in per.iter().flatten() {
            if !flat.contains(t) {
                flat.push(t.clone());
            }
        }
        eff.insert(id, flat);
        per_edge.insert(id, per);
    }

    // Start: the first node, resolved through spliced nodes.
    let start = nodes.first().and_then(|(id, _, _)| {
        let mut out = Vec::new();
        resolve(id, &edges, &spliced, &mut HashSet::new(), &mut out);
        out.into_iter().next()
    });

    // Priority: DFS preorder from the start, then from every node without incoming edges.
    let mut indeg: HashMap<&str, usize> = kept.iter().map(|&i| (nodes[i].0.as_str(), 0)).collect();
    for targets in eff.values() {
        for t in targets {
            if let Some(d) = indeg.get_mut(t.as_str()) {
                *d += 1;
            }
        }
    }
    let mut prio: HashMap<String, usize> = HashMap::new();
    let mut roots: Vec<String> = start.iter().cloned().collect();
    roots.extend(
        kept.iter()
            .map(|&i| nodes[i].0.clone())
            .filter(|id| indeg.get(id.as_str()) == Some(&0)),
    );
    roots.extend(kept.iter().map(|&i| nodes[i].0.clone()));
    for r in roots {
        let mut stack = vec![r];
        while let Some(n) = stack.pop() {
            if prio.contains_key(&n) {
                continue;
            }
            let p = prio.len();
            prio.insert(n.clone(), p);
            if let Some(next) = eff.get(n.as_str()) {
                for m in next.iter().rev() {
                    if !prio.contains_key(m) {
                        stack.push(m.clone());
                    }
                }
            }
        }
    }

    // Kahn's algorithm by priority; cycles are broken at the lowest-priority node.
    let mut order: Vec<String> = Vec::with_capacity(kept.len());
    let mut done: HashSet<String> = HashSet::new();
    let mut remaining = indeg.clone();
    let mut heap: BinaryHeap<Reverse<(usize, String)>> = remaining
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| Reverse((prio[*id], id.to_string())))
        .collect();
    while order.len() < kept.len() {
        let next = match heap.pop() {
            Some(Reverse((_, id))) if !done.contains(&id) => id,
            Some(_) => continue,
            None => {
                let Some(id) = remaining
                    .keys()
                    .filter(|id| !done.contains(**id))
                    .min_by_key(|id| prio[**id])
                    .map(|id| id.to_string())
                else {
                    break;
                };
                id
            }
        };
        done.insert(next.clone());
        if let Some(targets) = eff.get(next.as_str()) {
            for t in targets {
                if let Some(d) = remaining.get_mut(t.as_str()) {
                    *d = d.saturating_sub(1);
                    if *d == 0 && !done.contains(t) {
                        heap.push(Reverse((prio[t.as_str()], t.clone())));
                    }
                }
            }
        }
        order.push(next);
    }

    let out = order
        .iter()
        .map(|id| {
            let (_, short, node) = &nodes[by_id[id.as_str()]];
            build_node(id, short, node, &eff, &per_edge, t, texts)
        })
        .collect();
    (start, out)
}

fn build_node(
    id: &str,
    short: &str,
    node: &Value,
    eff: &HashMap<&str, Vec<String>>,
    per_edge: &HashMap<&str, Vec<Vec<String>>>,
    t: &DialogTables,
    texts: &Texts,
) -> NodeOut {
    let next = eff.get(id).cloned().unwrap_or_default();
    let mut out = NodeOut {
        id: id.to_string(),
        kind: "transition",
        action: None,
        speaker: None,
        text: None,
        alt_text: None,
        line_id: None,
        emotion: None,
        options: None,
        reference: None,
        next,
    };
    let lower_first = |s: &str| {
        let mut c = s.chars();
        c.next()
            .map(|f| f.to_lowercase().chain(c).collect::<String>())
            .unwrap_or_default()
    };
    match short {
        "Trunk" | "2DContent" | "ExTrunkReplace" => {
            let trunk = find_str(node, &["contentTrunkId", "_trunkId", "trunkId"]);
            let Some(trunk) = trunk else {
                out.action = Some("empty".into());
                return out;
            };
            out.kind = "line";
            out.line_id = Some(trunk.to_string());
            if let Some(line) = t.lines.get(trunk) {
                let name = texts.plain(line.actor_name);
                if !name.is_empty() || !line.actor_name_id.is_empty() {
                    out.speaker = Some(Speaker {
                        name,
                        id: line.actor_name_id.clone(),
                        icon: t.avatars.get(&line.actor_name_id).cloned(),
                    });
                }
                if let Some(raw) = texts.raw(line.dialog_text) {
                    out.text = Some(texts.rich_str(raw, None, texts.gender));
                    if has_gender_variants(raw) {
                        let alt = texts.rich_str(raw, None, texts.gender.other());
                        if Some(&alt) != out.text.as_ref() {
                            out.alt_text = Some(alt);
                        }
                    }
                }
                out.emotion = Some(line.emotion_type.clone()).filter(|e| !e.is_empty());
            }
        }
        "Option" | "ExOption" => {
            out.kind = "option";
            let mut ids = Vec::new();
            collect_strs(node, "_optionId", &mut ids);
            let per = per_edge.get(id).cloned().unwrap_or_default();
            out.options = Some(
                ids.into_iter()
                    .enumerate()
                    .map(|(i, oid)| OptionOut {
                        text: t
                            .options
                            .get(&oid)
                            .map(|o| texts.rich(o.option_text, None))
                            .unwrap_or_default(),
                        next: per.get(i).and_then(|v| v.first().cloned()),
                        id: oid,
                    })
                    .collect(),
            );
        }
        "Finish" => out.kind = "finish",
        "ExSummary" => {
            out.action = Some("summary".into());
            if let Some(sid) = find_str(node, &["summaryId"]) {
                out.reference = Some(sid.to_string());
                if let Some(r) = t.summaries.get(sid) {
                    out.text = Some(texts.rich(*r, None)).filter(|r| !r.is_empty());
                }
            }
        }
        "Dialog" => {
            out.action = Some("dialog".into());
            out.reference = find_str(node, &["_dialogId", "_name"]).map(str::to_string);
        }
        "Cinematic" => {
            out.action = Some("cinematic".into());
            out.reference = find_str(node, &["name"]).map(str::to_string);
        }
        other => out.action = Some(lower_first(other)),
    }
    out
}

/// Replace the bare `NaN` / `Infinity` / `-Infinity` tokens Unity's serializer emits
/// (invalid JSON) with `null`, leaving string contents alone.
pub fn replace_non_finite(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let (mut in_str, mut escaped) = (false, false);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_str = true;
            out.push(b);
            i += 1;
            continue;
        }
        let rest = &bytes[i..];
        if let Some(tok) = [&b"-Infinity"[..], b"Infinity", b"NaN"]
            .into_iter()
            .find(|t| rest.starts_with(t))
        {
            out.extend_from_slice(b"null");
            i += tok.len();
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

/// `GET /api/story/{dialogId}?lang=&gender=`
pub async fn dialog(
    State(state): State<SharedState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<LangQuery>,
) -> ApiResult<Json<DialogOut>> {
    let (lang, gender) = q.parse()?;
    let ix = story_index(&state).await?;
    let entry = ix
        .by_id
        .get(&id)
        .map(|&i| ix.entries[i].clone())
        .ok_or_else(|| ApiError::not_found(format!("no dialog {id:?}")))?;
    let dir = tables_dir(&state);
    let tables = &state.catalog.tables;
    let lines = tables.get::<DialogTextTable>(&dir).await?;
    let options = tables.get_or_default::<DialogOptionTable>(&dir).await?;
    let summaries = tables.get_or_default::<DialogSummaryTable>(&dir).await?;
    let actors = tables.get_or_default::<ActorImageTable>(&dir).await?;
    let text = tables.get_or_default::<TextTable>(&dir).await?;
    let texts = texts(&state, &lang, gender).await?;
    let path: PathBuf = Root::Gamedata.dir(&state.config.output).join(&entry.rel);
    let textures = Root::Textures.dir(&state.config.output);
    let out = blocking(move || -> ApiResult<DialogOut> {
        let bytes = std::fs::read(&path).map_err(|e| ApiError::io(e, "dialog"))?;
        let tree: Value = serde_json::from_slice(&bytes)
            .or_else(|_| serde_json::from_slice(&replace_non_finite(&bytes)))
            .map_err(|e| ApiError::internal(format!("dialog {}: {e}", entry.id)))?;
        let avatars: HashMap<String, String> = actors
            .iter()
            .filter_map(|(char_id, a)| {
                let code = char_id.splitn(3, '_').nth(2)?;
                let icon = assets::ui_png(&textures, "charicon", &a.avatar_path)?;
                Some((code.to_string(), icon))
            })
            .collect();
        let t = DialogTables {
            lines: &lines,
            options: &options,
            summaries: &summaries,
            avatars: &avatars,
        };
        let (start, nodes) = flatten(&tree, &t, &texts);
        let title = (entry.category == "mission")
            .then(|| text.get(&format!("{}_name", entry.group)))
            .flatten()
            .and_then(|r| texts.plain_opt(*r));
        Ok(DialogOut {
            id: entry.id,
            category: entry.category,
            group: entry.group,
            title,
            lang: texts.lang.clone(),
            gender: match gender {
                Gender::Female => "f",
                Gender::Male => "m",
            },
            start,
            nodes,
        })
    })
    .await??;
    Ok(Json(out))
}
