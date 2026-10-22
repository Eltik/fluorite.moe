//! Environment-driven configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::core::s3::{S3Settings, flag};

use super::paths::Root;

/// Server configuration. Every field has an env override:
///
/// | env | default |
/// |---|---|
/// | `FLUORITE_API_ADDR` | `127.0.0.1:8787` |
/// | `FLUORITE_OUTPUT` | `./output`, else `<crate>/output` |
/// | `FLUORITE_CACHE_DIR` | `<output>/.cache` |
/// | `FLUORITE_STATE` | `<output>/../data/update-state.json` |
/// | `FLUORITE_INDEX_POLL_SECS` | `60` |
/// | `FLUORITE_AUDIO_REMOTE` | `1` (read voice packages from the game CDN; `0` disables) |
/// | `FLUORITE_AUDIO_DIRS` | extra directories of `.pck` files (path-list separated) |
/// | `FLUORITE_AUDIO_CACHE_MB` | `5120` (transcoded clips beyond this are swept, oldest first) |
/// | `S3_*` | object storage for the roots in `S3_ROOTS` (unset: local disk) |
///
/// The S3 endpoint decides the provider (Cloudflare R2, Backblaze B2, DigitalOcean,
/// ...), and the provider supplies the settings that are not given: the signing
/// region, how the bucket is addressed, and whether the API redirects to the CDN in
/// front of it. An environment variable always wins over the derived value.
#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub output: PathBuf,
    pub cache_dir: PathBuf,
    pub state_file: PathBuf,
    pub index_poll: Duration,
    pub audio_remote: bool,
    pub audio_dirs: Vec<PathBuf>,
    /// Size cap of the transcoded clip cache, in bytes.
    pub audio_cache_max: u64,
    /// Object storage settings, when a bucket is configured.
    pub s3: Option<S3Config>,
}

/// How the API serves a root that lives in the bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Mode {
    /// Stream the object through the API.
    Proxy,
    /// Answer `302` with the object's public URL.
    Redirect,
}

impl S3Mode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "proxy" => Some(Self::Proxy),
            "redirect" => Some(Self::Redirect),
            _ => None,
        }
    }

    /// The name `S3_MODE` takes, for the logs.
    pub fn name(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Redirect => "redirect",
        }
    }
}

/// Object storage settings, read from the `S3_*` and `CLOUDFLARE_*` variables.
///
/// The bucket mirrors the local output layout: the key of `textures/ui/logo.png` is
/// `fluorite/textures/ui/logo.png` (below `prefix`), so the sync is a plain directory
/// mirror.
///
/// The settings themselves live in [`S3Settings`], which the unpacker reads too: the API
/// serves the roots of the bucket and the unpacker publishes into it, so both take the
/// endpoint, the credentials and the prefix from the same place. What is added here is
/// what only the API does with them: how a remote root is served, and whether the
/// thumbnails come from the bucket as well.
#[derive(Debug, Clone)]
pub struct S3Config {
    /// The endpoint, the credentials, the prefix and the roots of the bucket.
    pub settings: S3Settings,
    /// How a root that lives in the bucket is served.
    pub mode: S3Mode,
    /// Serve pre-generated thumbnails from the bucket.
    pub thumbs: bool,
}

impl S3Config {
    /// Read the S3 settings from the environment.
    ///
    /// `None` when no S3 variable is set at all: the API reads the local disk, as
    /// before. A half-filled configuration is an error rather than a silent fallback,
    /// so a typo cannot quietly fill the server's disk again.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let Some(settings) = S3Settings::from_env(&var)? else {
            return Ok(None);
        };

        let mode = match var("S3_MODE") {
            Some(value) => S3Mode::parse(&value)
                .ok_or_else(|| format!("S3_MODE: {value:?} is not 'proxy' or 'redirect'"))?,
            // A public base URL is a CDN in front of the bucket: the API answers with
            // the object's URL and the bytes never travel through the server.
            None if settings.public_base_url.is_some() => S3Mode::Redirect,
            None => S3Mode::Proxy,
        };

        Ok(Some(Self {
            settings,
            mode,
            thumbs: flag(&var, "S3_THUMBS", true),
        }))
    }

    /// The roots served from the bucket.
    pub fn roots(&self) -> &[Root] {
        &self.settings.roots
    }

    /// A one-line summary of the storage setup, for the startup log.
    pub fn describe(&self) -> String {
        let settings = &self.settings;
        let public = settings
            .public_base_url
            .as_deref()
            .map(|url| format!(", public {url}"))
            .unwrap_or_default();
        let roots: Vec<&str> = self.roots().iter().map(|root| root.rel_dir()).collect();
        format!(
            "{} bucket {} at {} (prefix {}, {}, {}, region {}{public}), roots: {}",
            settings.provider.name(),
            settings.bucket,
            settings.endpoint,
            settings.prefix_label(),
            self.mode.name(),
            settings.style_label(),
            settings.region,
            roots.join(", ")
        )
    }
}

impl Config {
    /// Config rooted at `output` with all other values derived from it.
    pub fn for_output(output: impl Into<PathBuf>) -> Self {
        let output = output.into();
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 8787)),
            cache_dir: output.join(".cache"),
            state_file: output.join("..").join("data").join("update-state.json"),
            index_poll: Duration::from_secs(60),
            audio_remote: true,
            audio_dirs: Vec::new(),
            audio_cache_max: 5 << 30,
            s3: None,
            output,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let var = crate::envfile::var;

        let output = match var("FLUORITE_OUTPUT") {
            Some(v) => PathBuf::from(v),
            None => {
                let cwd = PathBuf::from("output");
                if cwd.is_dir() {
                    cwd
                } else {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output")
                }
            }
        };
        let mut config = Self::for_output(output);
        if let Some(v) = var("FLUORITE_API_ADDR") {
            config.addr = v
                .parse()
                .map_err(|e| format!("FLUORITE_API_ADDR={v:?}: {e}"))?;
        }
        if let Some(v) = var("FLUORITE_CACHE_DIR") {
            config.cache_dir = PathBuf::from(v);
        }
        if let Some(v) = var("FLUORITE_STATE") {
            config.state_file = PathBuf::from(v);
        }
        if let Some(v) = var("FLUORITE_INDEX_POLL_SECS") {
            let secs: u64 = v
                .parse()
                .map_err(|e| format!("FLUORITE_INDEX_POLL_SECS={v:?}: {e}"))?;
            config.index_poll = Duration::from_secs(secs.max(1));
        }
        if let Some(v) = var("FLUORITE_AUDIO_REMOTE") {
            config.audio_remote = !matches!(v.trim(), "0" | "false" | "off" | "no");
        }
        if let Some(v) = var("FLUORITE_AUDIO_DIRS") {
            config.audio_dirs = std::env::split_paths(&v).collect();
        }
        if let Some(v) = var("FLUORITE_AUDIO_CACHE_MB") {
            let mb: u64 = v
                .parse()
                .map_err(|e| format!("FLUORITE_AUDIO_CACHE_MB={v:?}: {e}"))?;
            config.audio_cache_max = mb.saturating_mul(1 << 20);
        }
        config.s3 = S3Config::from_env(var)?;
        Ok(config)
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    pub fn update_log(&self) -> PathBuf {
        self.output.join("update-log.jsonl")
    }
}
