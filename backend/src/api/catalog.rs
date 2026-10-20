//! `/api/catalog/*`: game-data catalogs (operators, weapons, enemies, items, NPCs,
//! facilities) with localized text, plus the shared machinery the story reader uses.
//!
//! Caching works in three layers:
//! 1. [`tables::TableCache`]: each table JSON parsed once into typed structs on a blocking
//!    thread, re-parsed when its size/mtime changes.
//! 2. Per-kind *base* lists (language independent: ids, codes, icon paths verified on disk,
//!    model availability), memoized on the identity of the tables they were built from.
//! 3. Per-(kind, lang, gender) resolved lists, memoized on the base list and the i18n maps
//!    (which [`crate::api::i18n::I18nCache`] invalidates by mtime). LRU-bounded.
//!
//! Details are built per request from the cached tables (a single entity, sub-millisecond).

pub mod assets;
pub mod enemies;
pub mod facilities;
pub mod items;
pub mod npcs;
pub mod operators;
pub mod tables;
pub mod text;
pub mod weapons;

use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::Json;
use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::assets::{MeshIndex, RigIndex};
use self::tables::{AnyArc, HyperlinkTextTable, RichTextStyleTable, TableCache};
use self::text::{Gender, Markup, TermDef, TextRef, Texts};
use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::i18n::{TextMap, normalize_lang};
use super::paths::Root;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{kind}", get(list))
        .route("/{kind}/{id}", get(detail))
}

// ── Kinds ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Operators,
    Weapons,
    Enemies,
    Items,
    Npcs,
    Facilities,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Operators,
        Kind::Weapons,
        Kind::Enemies,
        Kind::Items,
        Kind::Npcs,
        Kind::Facilities,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Operators => "operators",
            Kind::Weapons => "weapons",
            Kind::Enemies => "enemies",
            Kind::Items => "items",
            Kind::Npcs => "npcs",
            Kind::Facilities => "facilities",
        }
    }

    pub fn parse(s: &str) -> ApiResult<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.name() == s)
            .ok_or_else(|| ApiError::not_found(format!("unknown catalog kind {s:?}")))
    }
}

// ── Memoization ──────────────────────────────────────────────────────────

struct MemoEntry<V> {
    deps: Vec<AnyArc>,
    built: Instant,
    used: Instant,
    value: Arc<V>,
}

/// LRU map whose entries are valid while their dependencies are the very same `Arc`s
/// (pointer identity; the entry holds them, so no address reuse) and, optionally, younger
/// than `ttl`.
pub struct Memo<K, V> {
    capacity: usize,
    ttl: Option<Duration>,
    map: std::sync::Mutex<HashMap<K, MemoEntry<V>>>,
}

impl<K: Eq + Hash + Clone, V> Memo<K, V> {
    pub fn new(capacity: usize, ttl: Option<Duration>) -> Self {
        Self {
            capacity: capacity.max(1),
            ttl,
            map: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &K, deps: &[AnyArc]) -> Option<Arc<V>> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let e = map.get_mut(key)?;
        let fresh = e.deps.len() == deps.len()
            && e.deps.iter().zip(deps).all(|(a, b)| Arc::ptr_eq(a, b))
            && self.ttl.is_none_or(|t| e.built.elapsed() < t);
        if !fresh {
            map.remove(key);
            return None;
        }
        e.used = Instant::now();
        Some(e.value.clone())
    }

    pub fn put(&self, key: K, deps: Vec<AnyArc>, value: Arc<V>) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.capacity
            && !map.contains_key(&key)
            && let Some(oldest) = map
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        let now = Instant::now();
        map.insert(
            key,
            MemoEntry {
                deps,
                built: now,
                used: now,
                value,
            },
        );
    }
}

pub fn dep<T: Send + Sync + 'static>(a: &Arc<T>) -> AnyArc {
    a.clone()
}

/// Run blocking work off the async runtime.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

// ── Shared list shapes ───────────────────────────────────────────────────

/// Tag display name source.
#[derive(Debug, Clone)]
pub enum TagName {
    Text(TextRef),
    Literal(String),
}

#[derive(Debug, Clone)]
pub struct BaseTag {
    pub kind: &'static str,
    pub code: String,
    pub name: TagName,
}

impl BaseTag {
    pub fn text(kind: &'static str, code: impl Into<String>, name: TextRef) -> Self {
        Self {
            kind,
            code: code.into(),
            name: TagName::Text(name),
        }
    }
}

/// Language-independent list entry.
#[derive(Debug, Clone)]
pub struct BaseItem {
    pub id: String,
    pub name: TextRef,
    /// Used when `name` resolves to nothing.
    pub fallback_name: String,
    pub rarity: Option<u8>,
    pub tags: Vec<BaseTag>,
    pub icon: Option<String>,
    pub has_model: bool,
    /// Filter value for `?type=`.
    pub type_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub kind: &'static str,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity: Option<u8>,
    pub tags: Vec<Tag>,
    /// Textures-root relative PNG (`/files/textures/<icon>`, `/thumb/<icon>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub has_model: bool,
    #[serde(skip)]
    search: String,
    #[serde(skip)]
    type_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Facet {
    pub code: String,
    pub name: String,
    pub count: usize,
}

pub struct ResolvedList {
    items: Vec<ListItem>,
    types: Vec<Facet>,
}

fn resolve_tag(t: &BaseTag, texts: &Texts) -> Tag {
    let name = match &t.name {
        TagName::Text(r) => texts.plain(*r),
        TagName::Literal(s) => s.clone(),
    };
    Tag {
        kind: t.kind,
        name: if name.is_empty() {
            t.code.clone()
        } else {
            name
        },
        code: t.code.clone(),
    }
}

fn resolve_list(base: &[BaseItem], texts: &Texts) -> ResolvedList {
    let mut facet: HashMap<String, Facet> = HashMap::new();
    let items: Vec<ListItem> = base
        .iter()
        .map(|b| {
            let mut name = texts.plain(b.name);
            if name.is_empty() {
                name = b.fallback_name.clone();
            }
            let tags: Vec<Tag> = b.tags.iter().map(|t| resolve_tag(t, texts)).collect();
            if let Some(code) = &b.type_code {
                let display = b
                    .tags
                    .iter()
                    .zip(&tags)
                    .find(|(bt, _)| &bt.code == code)
                    .map_or_else(|| code.clone(), |(_, t)| t.name.clone());
                facet
                    .entry(code.clone())
                    .or_insert_with(|| Facet {
                        code: code.clone(),
                        name: display,
                        count: 0,
                    })
                    .count += 1;
            }
            ListItem {
                search: format!("{}\n{}", name.to_lowercase(), b.id.to_lowercase()),
                id: b.id.clone(),
                name,
                rarity: b.rarity,
                tags,
                icon: b.icon.clone(),
                has_model: b.has_model,
                type_code: b.type_code.clone(),
            }
        })
        .collect();
    let mut types: Vec<Facet> = facet.into_values().collect();
    types.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.code.cmp(&b.code)));
    ResolvedList { items, types }
}

/// Sort helper: rarity desc, then id.
pub fn sort_rarity_desc(items: &mut [BaseItem]) {
    items.sort_by(|a, b| b.rarity.cmp(&a.rarity).then_with(|| a.id.cmp(&b.id)));
}

pub fn sort_id(items: &mut [BaseItem]) {
    items.sort_by(|a, b| a.id.cmp(&b.id));
}

// ── Environment handed to builders ───────────────────────────────────────

/// Paths and file-system indexes used while building (blocking context).
#[derive(Clone)]
pub struct Env {
    pub output: PathBuf,
    pub textures: PathBuf,
    pub meshes: Arc<MeshIndex>,
    pub rig: Arc<RigIndex>,
}

impl Env {
    pub fn ui(&self, dir: &str, name: &str) -> Option<String> {
        assets::ui_png(&self.textures, dir, name)
    }
}

// ── Catalog state ────────────────────────────────────────────────────────

/// Everything the catalog and story endpoints cache.
pub struct Catalog {
    pub tables: TableCache,
    bases: Memo<Kind, Vec<BaseItem>>,
    lists: Memo<(Kind, String, Gender), ResolvedList>,
    markup: Memo<(), Markup>,
    meshes: Memo<(), MeshIndex>,
    rig: std::sync::Mutex<Option<(Option<SystemTime>, Arc<RigIndex>)>>,
    pub story: crate::api::story::StoryCache,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

impl Catalog {
    pub fn new() -> Self {
        Self {
            tables: TableCache::new(),
            bases: Memo::new(16, Some(Duration::from_secs(300))),
            lists: Memo::new(48, None),
            markup: Memo::new(1, Some(Duration::from_secs(300))),
            meshes: Memo::new(1, Some(Duration::from_secs(120))),
            rig: std::sync::Mutex::new(None),
            story: crate::api::story::StoryCache::new(),
        }
    }
}

pub fn tables_dir(state: &SharedState) -> PathBuf {
    Root::Tables.dir(&state.config.output)
}

/// Mesh index (rebuilt every couple of minutes) and rigged-model index (by mtime).
pub async fn env(state: &SharedState) -> ApiResult<Env> {
    let output = state.config.output.clone();
    let cat = &state.catalog;
    let meshes = match cat.meshes.get(&(), &[]) {
        Some(m) => m,
        None => {
            let dir = Root::Meshes.dir(&output);
            let started = Instant::now();
            let m = Arc::new(blocking(move || MeshIndex::build(&dir)).await?);
            tracing::debug!("mesh index built in {} ms", started.elapsed().as_millis());
            cat.meshes.put((), Vec::new(), m.clone());
            m
        }
    };
    let index = Root::Models.dir(&output).join("index.json");
    let stamp = blocking(move || std::fs::metadata(&index).and_then(|m| m.modified()).ok()).await?;
    let cached = cat
        .rig
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(s, _)| *s == stamp)
        .map(|(_, r)| r.clone());
    let rig = match cached {
        Some(r) => r,
        None => {
            let out = output.clone();
            let r = Arc::new(blocking(move || RigIndex::load(&out)).await?);
            *cat.rig.lock().unwrap_or_else(|e| e.into_inner()) = Some((stamp, r.clone()));
            r
        }
    };
    Ok(Env {
        textures: Root::Textures.dir(&output),
        output,
        meshes,
        rig,
    })
}

/// Style colors and glossary terms (with verified icon paths).
async fn markup(state: &SharedState) -> ApiResult<Arc<Markup>> {
    let dir = tables_dir(state);
    let cat = &state.catalog;
    let styles = cat
        .tables
        .get_or_default::<RichTextStyleTable>(&dir)
        .await?;
    let links = cat
        .tables
        .get_or_default::<HyperlinkTextTable>(&dir)
        .await?;
    let deps = vec![dep(&styles), dep(&links)];
    if let Some(m) = cat.markup.get(&(), &deps) {
        return Ok(m);
    }
    let textures = Root::Textures.dir(&state.config.output);
    let m = Arc::new(
        blocking(move || Markup {
            styles: styles
                .iter()
                .filter_map(|(id, s)| Some((id.clone(), text::color_of(&s.pre_def)?)))
                .collect(),
            terms: links
                .iter()
                .map(|(id, l)| {
                    (
                        id.clone(),
                        TermDef {
                            name: l.name,
                            desc: l.desc,
                            icon: assets::ui_sprite_ref(&textures, &l.icon_path),
                            style: Some(l.rich_text_id.clone()).filter(|s| !s.is_empty()),
                        },
                    )
                })
                .collect(),
        })
        .await?,
    );
    cat.markup.put((), deps, m.clone());
    Ok(m)
}

/// i18n maps for `lang` plus the EN and CN fallbacks.
pub async fn text_maps(
    state: &SharedState,
    lang: &str,
) -> ApiResult<(Arc<TextMap>, Vec<Arc<TextMap>>)> {
    let dir = tables_dir(state);
    let primary = state.i18n.get(&dir, lang).await?;
    let mut fallbacks = Vec::new();
    for fb in ["EN", "CN"] {
        if fb != lang
            && let Ok(m) = state.i18n.get(&dir, fb).await
        {
            fallbacks.push(m);
        }
    }
    Ok((primary, fallbacks))
}

pub async fn texts(state: &SharedState, lang: &str, gender: Gender) -> ApiResult<Texts> {
    let (primary, fallbacks) = text_maps(state, lang).await?;
    let markup = markup(state).await?;
    Ok(Texts::new(
        lang.to_string(),
        gender,
        primary,
        fallbacks,
        markup,
    ))
}

// ── Per-kind dispatch ────────────────────────────────────────────────────

/// Language-independent list for `kind`, built once per table version.
async fn base(state: &SharedState, kind: Kind) -> ApiResult<Arc<Vec<BaseItem>>> {
    let dir = tables_dir(state);
    let cat = &state.catalog;
    let env = env(state).await?;
    macro_rules! build {
        ($m:ident) => {{
            let t = $m::Tables::load(&cat.tables, &dir).await?;
            let mut deps = t.deps();
            deps.push(dep(&env.meshes));
            if let Some(b) = cat.bases.get(&kind, &deps) {
                return Ok(b);
            }
            let started = Instant::now();
            let items = Arc::new(blocking(move || $m::base(&t, &env)).await?);
            tracing::info!(
                "built {} base list ({} entries) in {} ms",
                kind.name(),
                items.len(),
                started.elapsed().as_millis()
            );
            cat.bases.put(kind, deps, items.clone());
            Ok(items)
        }};
    }
    match kind {
        Kind::Operators => build!(operators),
        Kind::Weapons => build!(weapons),
        Kind::Enemies => build!(enemies),
        Kind::Items => build!(items),
        Kind::Npcs => build!(npcs),
        Kind::Facilities => build!(facilities),
    }
}

async fn detail_value(
    state: &SharedState,
    kind: Kind,
    id: String,
    texts: Texts,
) -> ApiResult<Option<Value>> {
    let dir = tables_dir(state);
    let cat = &state.catalog;
    let env = env(state).await?;
    macro_rules! build {
        ($m:ident) => {{
            let t = $m::Tables::load(&cat.tables, &dir).await?;
            blocking(move || $m::detail(&t, &env, &texts, &id)).await?
        }};
    }
    Ok(match kind {
        Kind::Operators => build!(operators),
        Kind::Weapons => build!(weapons),
        Kind::Enemies => build!(enemies),
        Kind::Items => build!(items),
        Kind::Npcs => build!(npcs),
        Kind::Facilities => build!(facilities),
    })
}

/// Serialize a detail struct and attach the glossary of terms it referenced.
pub fn finish_detail<T: Serialize>(value: &T, texts: &Texts) -> Value {
    let mut v = serde_json::to_value(value).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        let terms = texts.terms();
        o.insert(
            "terms".into(),
            serde_json::to_value(terms).unwrap_or(Value::Null),
        );
    }
    v
}

// ── Handlers ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct LangQuery {
    pub lang: Option<String>,
    pub gender: Option<String>,
}

impl LangQuery {
    pub fn parse(&self) -> ApiResult<(String, Gender)> {
        let lang = normalize_lang(self.lang.as_deref().unwrap_or("EN"))?;
        let gender = Gender::parse(self.gender.as_deref()).map_err(ApiError::bad_request)?;
        Ok((lang, gender))
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub lang: Option<String>,
    pub gender: Option<String>,
    pub q: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub const DEFAULT_LIMIT: usize = 1000;
pub const MAX_LIMIT: usize = 5000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexKind {
    pub kind: &'static str,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub kinds: Vec<IndexKind>,
}

/// `GET /api/catalog`
pub async fn index(State(state): State<SharedState>) -> ApiResult<Json<IndexResponse>> {
    let mut kinds = Vec::new();
    for kind in Kind::ALL {
        let count = base(&state, kind).await.map_or(0, |b| b.len());
        kinds.push(IndexKind {
            kind: kind.name(),
            count,
        });
    }
    Ok(Json(IndexResponse { kinds }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse<'a> {
    pub kind: &'static str,
    pub lang: String,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub items: Vec<&'a ListItem>,
    /// `?type=` values with counts (items only).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub types: &'a [Facet],
}

/// `GET /api/catalog/{kind}?lang=&gender=&q=&type=&limit=&offset=`
async fn list(
    State(state): State<SharedState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<axum::response::Response> {
    let kind = Kind::parse(&kind)?;
    let (lang, gender) = LangQuery {
        lang: q.lang.clone(),
        gender: q.gender.clone(),
    }
    .parse()?;
    let base = base(&state, kind).await?;
    let (primary, fallbacks) = text_maps(&state, &lang).await?;
    let mut deps = vec![dep(&base), dep(&primary)];
    deps.extend(fallbacks.iter().map(dep));
    let key = (kind, lang.clone(), gender);
    let resolved = match state.catalog.lists.get(&key, &deps) {
        Some(r) => r,
        None => {
            let markup = markup(&state).await?;
            let l = lang.clone();
            let b = base.clone();
            let r = Arc::new(
                blocking(move || {
                    let texts = Texts::new(l, gender, primary, fallbacks, markup);
                    resolve_list(&b, &texts)
                })
                .await?,
            );
            state.catalog.lists.put(key, deps, r.clone());
            r
        }
    };

    let needle =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
    let tokens: Vec<&str> = needle
        .as_deref()
        .map(|n| n.split_whitespace().collect())
        .unwrap_or_default();
    let type_filter = q.kind.as_deref().filter(|s| !s.is_empty());
    let matching: Vec<&ListItem> = resolved
        .items
        .iter()
        .filter(|it| type_filter.is_none_or(|t| it.type_code.as_deref() == Some(t)))
        .filter(|it| tokens.iter().all(|t| it.search.contains(t)))
        .collect();
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);
    let total = matching.len();
    let items: Vec<&ListItem> = matching.into_iter().skip(offset).take(limit).collect();
    let body = ListResponse {
        kind: kind.name(),
        lang,
        total,
        offset,
        limit,
        items,
        types: &resolved.types,
    };
    let bytes = serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?;
    json_bytes(bytes)
}

pub fn json_bytes(bytes: Vec<u8>) -> ApiResult<axum::response::Response> {
    axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/catalog/{kind}/{id}?lang=&gender=`
async fn detail(
    State(state): State<SharedState>,
    UrlPath((kind, id)): UrlPath<(String, String)>,
    Query(q): Query<LangQuery>,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&kind)?;
    let (lang, gender) = q.parse()?;
    let texts = texts(&state, &lang, gender).await?;
    let label = format!("{}/{id}", kind.name());
    detail_value(&state, kind, id, texts)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no catalog entry {label:?}")))
}

/// Shared helper: compact numeric output (3 decimals, integral values as integers).
pub fn num(v: f64) -> Value {
    let r = (v * 1000.0).round() / 1000.0;
    if r.fract() == 0.0 && r.abs() < 9.0e15 {
        Value::from(r as i64)
    } else {
        serde_json::Number::from_f64(r).map_or(Value::Null, Value::Number)
    }
}

/// Blackboard rows as a compact `{key: value}` map (string values win when present).
pub fn bb_json(rows: &[text::BbEntry]) -> serde_json::Map<String, Value> {
    rows.iter()
        .map(|r| {
            let v = if r.value_str.is_empty() {
                num(r.value)
            } else {
                Value::String(r.value_str.clone())
            };
            (r.key.clone(), v)
        })
        .collect()
}

/// Item reference with resolved display name, used by several details.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRef {
    pub id: String,
    pub count: i64,
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
