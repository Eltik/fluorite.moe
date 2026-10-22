//! The S3 services the API knows, and what each of them expects.
//!
//! The endpoint tells the service apart, so the details that differ per provider — the
//! signing region, whether the bucket is addressed as a path segment, and whether the
//! service puts a CDN in front of the bucket — are derived instead of configured. An
//! environment variable always wins over the derived value, so a provider can still be
//! corrected by hand.
//!
//! A provider that is not in the table is not an error: the endpoint keeps working as
//! a plain S3 endpoint, with the generic defaults, and the local disk stays the
//! fallback for every failed read.

/// An S3-compatible object store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Cloudflare R2.
    Cloudflare,
    /// Backblaze B2.
    Backblaze,
    /// Wasabi.
    Wasabi,
    /// DigitalOcean Spaces.
    DigitalOcean,
    /// Amazon S3.
    Aws,
    /// MinIO, or anything else on a local address.
    Minio,
    /// SeaweedFS.
    SeaweedFS,
    /// Any other service that speaks S3.
    Other,
}

impl Provider {
    /// The known services: the host suffix that identifies each of them.
    ///
    /// A new provider is one line here.
    const HOSTS: &'static [(Provider, &'static str)] = &[
        (Provider::Cloudflare, "r2.cloudflarestorage.com"),
        (Provider::Backblaze, "backblazeb2.com"),
        (Provider::Wasabi, "wasabisys.com"),
        (Provider::DigitalOcean, "digitaloceanspaces.com"),
        (Provider::Aws, "amazonaws.com"),
    ];

    /// The provider named by `S3_TYPE`.
    ///
    /// The names are the short ones the storage services are known by: `cloudflare`,
    /// `b2`, `minio`, `seaweedfs`, `aws`, `digitalocean`, `wasabi` and `s3` for
    /// everything else. The name wins over the host of the endpoint, so a service
    /// behind a custom domain can still be told what it is.
    pub fn from_type(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "cloudflare" | "r2" => Some(Self::Cloudflare),
            "b2" | "backblaze" => Some(Self::Backblaze),
            "wasabi" => Some(Self::Wasabi),
            "digitalocean" | "spaces" => Some(Self::DigitalOcean),
            "aws" | "amazon" => Some(Self::Aws),
            "minio" => Some(Self::Minio),
            "seaweedfs" | "seaweed" => Some(Self::SeaweedFS),
            "s3" | "other" | "generic" => Some(Self::Other),
            _ => None,
        }
    }

    /// The service behind `endpoint`, guessed from its host.
    pub fn detect(endpoint: &str) -> Self {
        let host = host_of(endpoint);
        for (provider, suffix) in Self::HOSTS {
            // The account or bucket name sits in front of the suffix:
            // `121a5.r2.cloudflarestorage.com` is Cloudflare, `notr2.cloudflarestorage.com`
            // is not.
            if host == *suffix
                || host
                    .strip_suffix(suffix)
                    .is_some_and(|rest| rest.ends_with('.'))
            {
                return *provider;
            }
        }
        if host == "localhost" || host.starts_with("127.") || host == "::1" {
            return Self::Minio;
        }
        if host.contains("minio") {
            return Self::Minio;
        }
        if host.contains("seaweed") {
            return Self::SeaweedFS;
        }
        Self::Other
    }

    /// The signing region the service expects.
    ///
    /// R2 signs with the fixed region `auto`, the services that put their location in
    /// the host name (B2, DigitalOcean) are read from it, and the rest take a plain
    /// region: `us-east-1`, the default of S3, which is also what MinIO and SeaweedFS
    /// answer to when they have no region of their own.
    pub fn region(&self, endpoint: &str) -> String {
        match self {
            Self::Cloudflare => "auto".to_string(),
            Self::Backblaze | Self::DigitalOcean => {
                region_in_host(endpoint).unwrap_or_else(|| "us-east-1".to_string())
            }
            Self::Wasabi | Self::Aws | Self::Minio | Self::SeaweedFS | Self::Other => {
                "us-east-1".to_string()
            }
        }
    }

    /// Whether the bucket is addressed as a path segment (`/<bucket>/<key>`) instead
    /// of a host prefix.
    pub fn path_style(&self) -> bool {
        // AWS addresses the bucket as a host prefix; the others accept the path form,
        // and some of them need it.
        !matches!(self, Self::Aws)
    }

    /// Whether the service puts a CDN in front of the bucket.
    ///
    /// With a CDN, the API answers with the object's public URL instead of streaming
    /// the bytes, and the server never carries them.
    pub fn has_cdn(&self) -> bool {
        matches!(self, Self::Cloudflare)
    }

    /// A short name for the logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cloudflare => "Cloudflare R2",
            Self::Backblaze => "Backblaze B2",
            Self::Wasabi => "Wasabi",
            Self::DigitalOcean => "DigitalOcean Spaces",
            Self::Aws => "Amazon S3",
            Self::Minio => "MinIO",
            Self::SeaweedFS => "SeaweedFS",
            Self::Other => "S3",
        }
    }
}

/// The host of an endpoint URL: no scheme, no path, no port, no user info.
fn host_of(endpoint: &str) -> String {
    let rest = match endpoint.find("://") {
        Some(i) => &endpoint[i + 3..],
        None => endpoint,
    };
    let rest = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    // An IPv6 host is bracketed: `[::1]:8333`.
    let host = match rest.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => rest.split(':').next().unwrap_or(rest),
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The location a service puts in its host name.
///
/// `s3.us-west-004.backblazeb2.com` is `us-west-004`, `space.nyc3.digitaloceanspaces.com`
/// is `nyc3`: the label in front of the service name.
fn region_in_host(endpoint: &str) -> Option<String> {
    let host = host_of(endpoint);
    let labels: Vec<&str> = host.split('.').collect();
    // The shape is `<service>.<location>.<provider>.<tld>`; anything shorter has no
    // location to read.
    if labels.len() < 4 {
        return None;
    }
    let region = labels[1];
    if !region
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        || !region.contains(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    Some(region.to_string())
}
