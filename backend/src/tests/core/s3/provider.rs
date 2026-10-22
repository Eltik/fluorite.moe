//! The tests of the providers: what the endpoint tells, and what the settings take from
//! it.

use crate::api::config::{S3Config, S3Mode};
use crate::core::s3::Provider;

use super::{base, env};

/// A configuration for one endpoint.
fn with_endpoint(endpoint: &'static str) -> Vec<(&'static str, &'static str)> {
    base(&[("S3_ENDPOINT", endpoint)])
}

#[test]
fn the_endpoint_tells_the_provider_apart() {
    let cases = [
        (
            "https://121a543e.r2.cloudflarestorage.com",
            Provider::Cloudflare,
        ),
        (
            "https://s3.us-west-004.backblazeb2.com",
            Provider::Backblaze,
        ),
        ("https://s3.wasabisys.com", Provider::Wasabi),
        (
            "https://space.nyc3.digitaloceanspaces.com",
            Provider::DigitalOcean,
        ),
        ("https://s3.eu-west-1.amazonaws.com", Provider::Aws),
        ("http://127.0.0.1:8333", Provider::Minio),
        ("http://localhost:9000", Provider::Minio),
        ("http://[::1]:9000", Provider::Minio),
        ("https://minio.example.com", Provider::Minio),
        ("https://seaweed.example.com", Provider::SeaweedFS),
        ("https://objects.example.com", Provider::Other),
    ];
    for (endpoint, expected) in cases {
        assert_eq!(
            Provider::detect(endpoint),
            expected,
            "{endpoint} is {expected:?}"
        );
    }
}

#[test]
fn a_lookalike_host_is_not_the_provider() {
    // The account name sits in front of the suffix; a name that merely ends with it
    // is somebody else's host.
    assert_eq!(
        Provider::detect("https://notr2.cloudflarestorage.com"),
        Provider::Other
    );
}

#[test]
fn the_region_follows_the_provider() {
    // R2 signs with the fixed region "auto".
    assert_eq!(
        Provider::Cloudflare.region("https://x.r2.cloudflarestorage.com"),
        "auto"
    );
    // B2 and DigitalOcean put their location in the host name.
    assert_eq!(
        Provider::Backblaze.region("https://s3.us-west-004.backblazeb2.com"),
        "us-west-004"
    );
    assert_eq!(
        Provider::DigitalOcean.region("https://space.nyc3.digitaloceanspaces.com"),
        "nyc3"
    );
    // A location that cannot be read falls back to a plain one.
    assert_eq!(
        Provider::Backblaze.region("https://backblazeb2.com"),
        "us-east-1"
    );
    // MinIO and the other plain endpoints sign with the default region of S3.
    assert_eq!(Provider::Minio.region("http://127.0.0.1:9000"), "us-east-1");
    assert_eq!(
        Provider::Other.region("https://objects.example.com"),
        "us-east-1"
    );
}

#[test]
fn the_bucket_is_addressed_the_way_the_provider_wants() {
    // AWS addresses the bucket as a host prefix, the others as a path segment.
    assert!(!Provider::Aws.path_style());
    assert!(Provider::Cloudflare.path_style());
    assert!(Provider::Backblaze.path_style());
}

#[test]
fn only_cloudflare_puts_a_cdn_in_front_of_the_bucket() {
    assert!(Provider::Cloudflare.has_cdn());
    assert!(!Provider::Minio.has_cdn());
    assert!(!Provider::Other.has_cdn());
}

#[test]
fn the_configuration_uses_the_detected_provider() {
    let cfg = S3Config::from_env(env(&with_endpoint(
        "https://121a543e.r2.cloudflarestorage.com",
    )))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Cloudflare);
    assert_eq!(cfg.settings.region, "auto");
    assert!(cfg.settings.path_style);
    // Without a public URL there is no CDN to redirect to: the API streams the bytes.
    assert_eq!(cfg.mode, S3Mode::Proxy);
}

#[test]
fn an_explicit_setting_wins_over_the_provider() {
    let cfg = S3Config::from_env(env(&base(&[
        ("S3_REGION", "eu-central-1"),
        ("S3_PATH_STYLE", "false"),
        ("S3_MODE", "proxy"),
    ])))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Cloudflare);
    assert_eq!(cfg.settings.region, "eu-central-1");
    assert!(!cfg.settings.path_style);
    assert_eq!(cfg.mode, S3Mode::Proxy);
}

#[test]
fn a_public_base_url_means_the_cdn_serves_the_bytes() {
    // The CDN in front of the bucket answers the request, so the API redirects to it
    // and the server never carries the bytes.
    let cfg = S3Config::from_env(env(&base(&[(
        "S3_PUBLIC_BASE_URL",
        "https://assets.eltik.net",
    )])))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.mode, S3Mode::Redirect);
    assert_eq!(
        cfg.settings.public_base_url.as_deref(),
        Some("https://assets.eltik.net")
    );

    // The mode can still be forced, for a bucket whose CDN is not the public one.
    let cfg = S3Config::from_env(env(&base(&[
        ("S3_PUBLIC_BASE_URL", "https://assets.eltik.net"),
        ("S3_MODE", "proxy"),
    ])))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.mode, S3Mode::Proxy);
}

#[test]
fn an_unknown_service_still_works_as_a_plain_endpoint() {
    let cfg = S3Config::from_env(env(&with_endpoint("https://objects.example.com")))
        .unwrap()
        .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Other);
    assert_eq!(cfg.settings.region, "us-east-1");
    assert!(cfg.settings.path_style);
    assert_eq!(cfg.mode, S3Mode::Proxy);
}

#[test]
fn the_summary_names_the_provider_and_the_mode() {
    let cfg = S3Config::from_env(env(&base(&[
        ("S3_ENDPOINT", "https://121a543e.r2.cloudflarestorage.com"),
        ("S3_PUBLIC_BASE_URL", "https://assets.eltik.net"),
    ])))
    .unwrap()
    .unwrap();
    let summary = cfg.describe();
    assert!(summary.contains("Cloudflare R2"), "{summary}");
    assert!(summary.contains("redirect"), "{summary}");
    assert!(summary.contains("assets"), "{summary}");
}

#[test]
fn the_cloudflare_variables_are_the_settings_of_r2() {
    // The names the Cloudflare dashboard hands out are the ones the R2 setup uses.
    let cfg = S3Config::from_env(env(&[
        ("S3_TYPE", "cloudflare"),
        (
            "CLOUDFLARE_S3_API",
            "https://121a543e.r2.cloudflarestorage.com",
        ),
        ("CLOUDFLARE_API_ACCESS_KEY", "AKID"),
        ("CLOUDFLARE_API_SECRET_KEY", "secret"),
        ("S3_BUCKET", "assets"),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Cloudflare);
    assert_eq!(
        cfg.settings.endpoint,
        "https://121a543e.r2.cloudflarestorage.com"
    );
    assert_eq!(cfg.settings.bucket, "assets");
    assert_eq!(cfg.settings.credentials.access_key_id, "AKID");
    assert_eq!(cfg.settings.credentials.secret_access_key, "secret");
    assert_eq!(cfg.settings.region, "auto");
    assert!(cfg.settings.path_style);
}

#[test]
fn the_account_id_builds_the_endpoint_when_it_is_not_given() {
    let cfg = S3Config::from_env(env(&[
        ("S3_TYPE", "cloudflare"),
        ("CLOUDFLARE_ACCOUNT_ID", "121a543e"),
        ("CLOUDFLARE_API_ACCESS_KEY", "AKID"),
        ("CLOUDFLARE_API_SECRET_KEY", "secret"),
        ("S3_BUCKET", "assets"),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(
        cfg.settings.endpoint,
        "https://121a543e.r2.cloudflarestorage.com"
    );
    assert_eq!(cfg.settings.provider, Provider::Cloudflare);
}

#[test]
fn the_generic_name_wins_over_the_cloudflare_one() {
    let cfg = S3Config::from_env(env(&[
        ("S3_TYPE", "cloudflare"),
        ("S3_ENDPOINT", "https://s3.example.com"),
        (
            "CLOUDFLARE_S3_API",
            "https://121a543e.r2.cloudflarestorage.com",
        ),
        ("S3_ACCESS_KEY_ID", "AKID"),
        ("CLOUDFLARE_API_ACCESS_KEY", "OTHER"),
        ("S3_SECRET_ACCESS_KEY", "secret"),
        ("CLOUDFLARE_API_SECRET_KEY", "OTHER"),
        ("S3_BUCKET", "assets"),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.settings.endpoint, "https://s3.example.com");
    assert_eq!(cfg.settings.credentials.access_key_id, "AKID");
    assert_eq!(cfg.settings.credentials.secret_access_key, "secret");
}

#[test]
fn the_service_named_in_s3_type_wins_over_the_host() {
    // A service behind a custom domain can still be told what it is.
    let cfg = S3Config::from_env(env(&base(&[("S3_TYPE", "cloudflare")])))
        .unwrap()
        .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Cloudflare);

    let cfg = S3Config::from_env(env(&base(&[("S3_TYPE", "s3")])))
        .unwrap()
        .unwrap();
    assert_eq!(cfg.settings.provider, Provider::Other);
}

#[test]
fn an_unknown_service_name_is_an_error() {
    let err = S3Config::from_env(env(&base(&[("S3_TYPE", "dropbox")]))).unwrap_err();
    assert!(err.contains("S3_TYPE"), "{err}");
    assert!(err.contains("dropbox"), "{err}");
}
