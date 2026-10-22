//! The object storage settings, read from the `S3_*` and `CLOUDFLARE_*` variables.
//!
//! The names are the ones a storage service uses for itself, so a dashboard export can
//! be pasted in as it is: Cloudflare writes `CLOUDFLARE_S3_API`,
//! `CLOUDFLARE_API_ACCESS_KEY` and `CLOUDFLARE_API_SECRET_KEY`, and every other service
//! is reached through the generic `S3_*` names. The generic name wins when both are set.

use super::client::{Credentials, S3Client};
use super::provider::Provider;
use crate::core::roots::Root;

/// The key prefix every object lives under, when `S3_PREFIX` is not set.
pub const DEFAULT_PREFIX: &str = "fluorite";

/// The roots that live in the bucket, when `S3_ROOTS` is not set: the heavy ones.
const DEFAULT_ROOTS: [Root; 3] = [Root::Textures, Root::Meshes, Root::Models];

/// How many objects are uploaded at once, when `S3_UPLOAD_CONCURRENCY` is not set.
const DEFAULT_UPLOAD_CONCURRENCY: usize = 16;

/// The object storage settings, read from the `S3_*` and `CLOUDFLARE_*` variables.
///
/// The API serves the roots that live in the bucket and the unpacker publishes into it,
/// so both read the same variables: the endpoint decides the provider, and the provider
/// supplies the settings that are not given (the signing region, how the bucket is
/// addressed). An environment variable always wins over the derived value.
///
/// | env | default |
/// |---|---|
/// | `S3_TYPE` | from the endpoint host |
/// | `S3_ENDPOINT` | required (or `CLOUDFLARE_S3_API`) |
/// | `S3_BUCKET` | required |
/// | `S3_ACCESS_KEY_ID` | required (or `CLOUDFLARE_API_ACCESS_KEY`) |
/// | `S3_SECRET_ACCESS_KEY` | required (or `CLOUDFLARE_API_SECRET_KEY`) |
/// | `S3_SESSION_TOKEN` | none |
/// | `S3_REGION` | from the provider |
/// | `S3_PATH_STYLE` | from the provider |
/// | `S3_PREFIX` | `fluorite` |
/// | `S3_ROOTS` | `textures,meshes,models` |
/// | `S3_PUBLIC_BASE_URL` | none |
/// | `S3_CACHE_CONTROL` | none |
/// | `S3_KEEP_LOCAL` | `1` (keep the local copy of an uploaded file) |
/// | `S3_UPLOAD_CONCURRENCY` | `16` |
#[derive(Clone)]
pub struct S3Settings {
    /// Endpoint URL, e.g. `https://<account>.r2.cloudflarestorage.com`.
    pub endpoint: String,
    pub bucket: String,
    /// The service behind the endpoint, detected from its host.
    pub provider: Provider,
    /// Signing region (`auto` for R2, `us-east-1` for MinIO, `us-west-004` for B2).
    pub region: String,
    pub credentials: Credentials,
    /// Key prefix inside the bucket, without slashes on either sides.
    pub prefix: String,
    /// The roots that live in the bucket.
    pub roots: Vec<Root>,
    /// Public base URL of the bucket; a CDN in front of it.
    pub public_base_url: Option<String>,
    /// The `Cache-Control` the objects are stored with.
    pub cache_control: Option<String>,
    /// Keep the local copy of a file after it was uploaded.
    pub keep_local: bool,
    /// How many objects are uploaded (and pruned) at once.
    pub upload_concurrency: usize,
    /// Address the bucket as a path segment (`/<bucket>/<key>`) instead of a host prefix.
    pub path_style: bool,
}

impl std::fmt::Debug for S3Settings {
    /// The secret is not printed: the settings end up in the logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Settings")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("provider", &self.provider)
            .field("region", &self.region)
            .field("prefix", &self.prefix)
            .field("roots", &self.roots)
            .field("public_base_url", &self.public_base_url)
            .field("cache_control", &self.cache_control)
            .field("keep_local", &self.keep_local)
            .field("upload_concurrency", &self.upload_concurrency)
            .field("path_style", &self.path_style)
            .field("access_key_id", &self.credentials.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .finish()
    }
}

impl S3Settings {
    /// Read the settings from the environment.
    ///
    /// `None` when no S3 variable is set at all: the API reads the local disk, as
    /// before. A half-filled configuration is an error rather than a silent fallback,
    /// so a typo cannot quietly fill the server's disk again.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        // The service, when it is named. The name wins over the host of the endpoint.
        let named = match var("S3_TYPE") {
            Some(name) => Some(
                Provider::from_type(&name)
                    .ok_or_else(|| format!("S3_TYPE: {name:?} is not a known service"))?,
            ),
            None => None,
        };
        // Cloudflare's own names are only read for the service they belong to.
        let cloudflare = |name: &str| {
            if named == Some(Provider::Cloudflare) {
                var(name)
            } else {
                None
            }
        };

        // The endpoint: the generic name, then Cloudflare's own, then the one the
        // account id builds.
        let endpoint = var("S3_ENDPOINT")
            .or_else(|| cloudflare("CLOUDFLARE_S3_API"))
            .or_else(|| {
                var("CLOUDFLARE_ACCOUNT_ID")
                    .map(|account| format!("https://{}.r2.cloudflarestorage.com", account.trim()))
            });
        let bucket = var("S3_BUCKET");
        let access_key_id =
            var("S3_ACCESS_KEY_ID").or_else(|| cloudflare("CLOUDFLARE_API_ACCESS_KEY"));
        let secret_access_key =
            var("S3_SECRET_ACCESS_KEY").or_else(|| cloudflare("CLOUDFLARE_API_SECRET_KEY"));

        let any = named.is_some()
            || [&endpoint, &bucket, &access_key_id, &secret_access_key]
                .iter()
                .any(|v| v.is_some());
        if !any {
            return Ok(None);
        }

        let need = |value: Option<String>, name: &str| -> Result<String, String> {
            value.ok_or_else(|| format!("{name} is required when the other S3 settings are set"))
        };
        let endpoint = need(endpoint, "S3_ENDPOINT (or CLOUDFLARE_S3_API)")?;
        let bucket = need(bucket, "S3_BUCKET")?;
        let access_key_id = need(
            access_key_id,
            "S3_ACCESS_KEY_ID (or CLOUDFLARE_API_ACCESS_KEY)",
        )?;
        let secret_access_key = need(
            secret_access_key,
            "S3_SECRET_ACCESS_KEY (or CLOUDFLARE_API_SECRET_KEY)",
        )?;

        let roots = match var("S3_ROOTS") {
            Some(list) => parse_roots(&list)?,
            None => DEFAULT_ROOTS.to_vec(),
        };
        let upload_concurrency = match var("S3_UPLOAD_CONCURRENCY") {
            Some(value) => parse_concurrency(&value)?,
            None => DEFAULT_UPLOAD_CONCURRENCY,
        };

        let provider = named.unwrap_or_else(|| Provider::detect(&endpoint));

        // What the provider decides, unless the environment says otherwise.
        let region = var("S3_REGION").unwrap_or_else(|| provider.region(&endpoint));
        let path_style = flag_opt(&var, "S3_PATH_STYLE").unwrap_or_else(|| provider.path_style());

        Ok(Some(Self {
            endpoint,
            bucket,
            provider,
            region,
            credentials: Credentials {
                access_key_id,
                secret_access_key,
                session_token: var("S3_SESSION_TOKEN"),
            },
            prefix: var("S3_PREFIX")
                .map(|prefix| prefix.trim_matches('/').to_string())
                .unwrap_or_else(|| DEFAULT_PREFIX.to_string()),
            roots,
            public_base_url: var("S3_PUBLIC_BASE_URL"),
            cache_control: var("S3_CACHE_CONTROL"),
            keep_local: flag_opt(&var, "S3_KEEP_LOCAL").unwrap_or(true),
            upload_concurrency,
            path_style,
        }))
    }

    /// A client for the bucket.
    pub fn client(&self) -> Result<S3Client, String> {
        S3Client::new(
            &self.endpoint,
            &self.region,
            &self.bucket,
            self.credentials.clone(),
            self.path_style,
        )
    }

    /// `path` below the key prefix (`<prefix>/<path>`, or just `<path>` without one).
    pub fn prefixed(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    /// The key prefix of a root's objects (`<prefix>/<root>/`).
    pub fn root_prefix(&self, root: Root) -> String {
        format!("{}/", self.prefixed(root.rel_dir()))
    }

    /// The key of a file below a root (`<prefix>/<root>/<path>`).
    pub fn key_for(&self, root: Root, rel: &str) -> String {
        let prefix = self.root_prefix(root);
        if rel.is_empty() {
            prefix.trim_end_matches('/').to_string()
        } else {
            format!("{prefix}{rel}")
        }
    }

    /// The prefix as the logs name it: `none` when there is none.
    pub fn prefix_label(&self) -> &str {
        if self.prefix.is_empty() {
            "none"
        } else {
            &self.prefix
        }
    }

    /// How the bucket is addressed, as the logs name it.
    pub fn style_label(&self) -> &'static str {
        if self.path_style {
            "path style"
        } else {
            "host style"
        }
    }

    /// A one-line summary of the storage setup, for the logs.
    pub fn describe(&self) -> String {
        let roots: Vec<&str> = self.roots.iter().map(|root| root.name()).collect();
        format!(
            "{} bucket {} at {} (prefix {}, {}, region {}), roots: {}",
            self.provider.name(),
            self.bucket,
            self.endpoint,
            self.prefix_label(),
            self.style_label(),
            self.region,
            roots.join(", ")
        )
    }
}

/// The roots named in `S3_ROOTS`: a comma-separated list, blanks ignored.
fn parse_roots(list: &str) -> Result<Vec<Root>, String> {
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Root::parse(name).ok_or_else(|| format!("S3_ROOTS: {name:?} is not a root")))
        .collect()
}

/// The number in `S3_UPLOAD_CONCURRENCY`: a whole number above zero.
fn parse_concurrency(value: &str) -> Result<usize, String> {
    value
        .trim()
        .parse()
        .ok()
        .filter(|count| *count > 0)
        .ok_or_else(|| format!("S3_UPLOAD_CONCURRENCY: {value:?} is not a positive number"))
}

/// Read a boolean variable (`1`/`0`, `true`/`false`, `on`/`off`), or `None` when it is
/// not set.
pub fn flag_opt(var: &impl Fn(&str) -> Option<String>, name: &str) -> Option<bool> {
    var(name).map(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// Read a boolean variable, with a default when it is not set.
pub fn flag(var: &impl Fn(&str) -> Option<String>, name: &str, default: bool) -> bool {
    flag_opt(var, name).unwrap_or(default)
}
