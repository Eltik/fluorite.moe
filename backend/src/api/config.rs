//! Environment-driven configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

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
            output,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());

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
        Ok(config)
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    pub fn update_log(&self) -> PathBuf {
        self.output.join("update-log.jsonl")
    }
}
