//! The S3 client: AWS Signature V4 request signing and the object operations.
//!
//! Written by hand so the backend keeps its small dependency tree and can talk to any
//! S3-compatible endpoint: Cloudflare R2, Backblaze B2, DigitalOcean Spaces, or MinIO
//! on an Oracle Cloud Always Free instance. Only `sha2` and `hmac` are added for the
//! signature; transport reuses `reqwest`, timestamps reuse `chrono` and `httpdate`.
//!
//! The reads serve the API; the writes publish the extraction output into the bucket,
//! so the two halves of the project (the server and the unpacker) share one client.
//!
//! A request without a body is signed with the empty-payload hash; an upload is signed
//! with the SHA-256 of the bytes it carries, which is why the body is read twice (once
//! to hash it, once to send it) instead of being kept in memory.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
/// SHA-256 of the empty body.
const EMPTY_PAYLOAD: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// `ListObjectsV2` caps a page at 1000 keys.
const MAX_KEYS: u32 = 1000;
/// The chunk an uploaded file is read in.
const READ_CHUNK: usize = 1 << 20;
/// The signing region when none is given: the default of S3.
const DEFAULT_REGION: &str = "us-east-1";
/// How long a write may go without an answer before it is given up as stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// The slowest upload that still counts as progress, in bytes per second: an upload
/// gets [`STALL_TIMEOUT`] plus the time its file takes at this rate, so a large file on
/// a busy link is not cut off.
const MIN_UPLOAD_RATE: u64 = 64 * 1024;

/// Credentials for a bucket. `session_token` is only set for temporary (STS) keys.
#[derive(Clone)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// A failed S3 call: the HTTP status and whatever the service said about it.
#[derive(Debug, Clone)]
pub struct S3Error {
    pub status: u16,
    pub message: String,
    /// The local file could not be read: asking the service again does not help.
    local: bool,
}

impl S3Error {
    pub fn not_found(key: &str) -> Self {
        Self {
            status: 404,
            message: format!("object not found: {key}"),
            local: false,
        }
    }

    /// Whether the same call may succeed when it is made again: the service was busy
    /// (`429`) or failing (`5xx`), or its answer never came (a connection, a timeout).
    pub fn is_transient(&self) -> bool {
        match self.status {
            0 => !self.local,
            429 => true,
            status => (500..600).contains(&status),
        }
    }

    /// A failure before the service answered (a connection, a read of the local
    /// file): there is no HTTP status, so it is reported as `0`.
    fn transport(message: impl ToString) -> Self {
        Self {
            status: 0,
            message: message.to_string(),
            local: false,
        }
    }

    /// A local file that could not be read for an upload.
    fn file(path: &Path, error: std::io::Error) -> Self {
        Self {
            local: true,
            ..Self::transport(format!("{}: {error}", path.display()))
        }
    }

    /// A write that got no answer within `budget`.
    fn stalled(budget: Duration) -> Self {
        Self::transport(format!("no answer within {}s", budget.as_secs()))
    }
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S3 {}: {}", self.status, self.message)
    }
}

impl std::error::Error for S3Error {}

/// Metadata of one object, as returned by `HEAD`.
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub size: u64,
    /// The object's entity tag, without the surrounding quotes.
    pub etag: String,
    pub last_modified: Option<DateTime<Utc>>,
}

/// An in-flight object body plus the headers that came with it.
pub struct ObjectStream {
    pub status: u16,
    pub etag: String,
    pub last_modified: Option<DateTime<Utc>>,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
    pub body: reqwest::Response,
}

/// One object in a listing.
#[derive(Debug, Clone)]
pub struct ListedObject {
    pub key: String,
    pub size: u64,
    pub etag: String,
    pub last_modified: Option<DateTime<Utc>>,
}

/// One page of a `ListObjectsV2` call.
#[derive(Debug, Clone, Default)]
pub struct ListResult {
    pub objects: Vec<ListedObject>,
    /// Common prefixes (the "directories" under `prefix`).
    pub prefixes: Vec<String>,
    pub next_continuation: Option<String>,
}

/// An S3-compatible endpoint.
pub struct S3Client {
    http: reqwest::Client,
    endpoint: Url,
    region: String,
    bucket: String,
    credentials: Credentials,
    path_style: bool,
}

impl S3Client {
    /// Build a client for `bucket` at `endpoint`.
    ///
    /// The endpoint is a URL such as `https://<account>.r2.cloudflarestorage.com`; a
    /// trailing path is kept (MinIO behind a reverse proxy lives under one). With
    /// `path_style` the bucket is a path segment (`/<bucket>/<key>`), otherwise it is a
    /// host prefix (`<bucket>.<host>/<key>`).
    pub fn new(
        endpoint: &str,
        region: &str,
        bucket: &str,
        credentials: Credentials,
        path_style: bool,
    ) -> Result<Self, String> {
        let trimmed = endpoint.trim();
        let with_scheme = if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        };
        let endpoint = Url::parse(&with_scheme).map_err(|e| format!("bad S3 endpoint: {e}"))?;
        if endpoint.host_str().is_none() {
            return Err("S3 endpoint has no host".to_string());
        }
        if bucket.is_empty() {
            return Err("S3 bucket is empty".to_string());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("S3 client: {e}"))?;
        let region = match region.trim() {
            "" => DEFAULT_REGION,
            region => region,
        };
        Ok(Self {
            http,
            endpoint,
            region: region.to_string(),
            bucket: bucket.to_string(),
            credentials,
            path_style,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The path part of the endpoint, without a trailing slash ("" for a bare host).
    fn endpoint_path(&self) -> String {
        self.endpoint.path().trim_end_matches('/').to_string()
    }

    /// `host` or `host:port`, as it appears in the `Host` header.
    fn host(&self) -> String {
        let host = self.endpoint.host_str().unwrap_or_default();
        match self.endpoint.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }
    }

    /// The authority the request goes to (the bucket is a host prefix in virtual style).
    fn authority(&self) -> String {
        if self.path_style {
            self.host()
        } else {
            format!("{}.{}", self.bucket, self.host())
        }
    }

    /// The canonical path prefix of every object URL (`/<bucket>` in path style).
    fn base_path(&self) -> String {
        let prefix = self.endpoint_path();
        if self.path_style {
            format!("{prefix}/{}", uri_encode(&self.bucket, false))
        } else {
            prefix
        }
    }

    /// The URL and the canonical path of one object.
    fn object_url(&self, key: &str) -> (String, String) {
        let encoded = uri_encode(key, false);
        let path = format!("{}/{}", self.base_path(), encoded);
        let url = format!("{}://{}{}", self.endpoint.scheme(), self.authority(), path);
        (url, path)
    }

    /// Sign a request and send it.
    ///
    /// `extra_headers` are sent and signed (the `Range` header, for instance); the
    /// canonical query is built from `query`, sorted and encoded. `payload_hash` is the
    /// SHA-256 of the body: the empty one for the calls without a body.
    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        query: &[(String, String)],
        extra_headers: &[(String, String)],
        payload_hash: &str,
        body: Option<reqwest::Body>,
    ) -> Result<reqwest::Response, S3Error> {
        let (base_url, canonical_path) = self.object_url(key);
        let now = Utc::now();

        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), self.authority()),
            ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ];
        if let Some(token) = &self.credentials.session_token {
            headers.push(("x-amz-security-token".to_string(), token.clone()));
        }
        for (name, value) in extra_headers {
            headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
        }

        let signed = Signer {
            secret: &self.credentials.secret_access_key,
            access_key_id: &self.credentials.access_key_id,
            region: &self.region,
        }
        .sign(
            method.as_str(),
            &canonical_path,
            query,
            &headers,
            payload_hash,
            now,
        );

        // The query is already encoded and sorted, so it can be appended verbatim.
        let url = if signed.canonical_query.is_empty() {
            base_url
        } else {
            format!("{base_url}?{}", signed.canonical_query)
        };

        let mut request = self
            .http
            .request(method, &url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", payload_hash)
            .header("authorization", &signed.authorization);
        if let Some(token) = &self.credentials.session_token {
            request = request.header("x-amz-security-token", token);
        }
        for (name, value) in extra_headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(body) = body {
            request = request.body(body);
        }

        request.send().await.map_err(S3Error::transport)
    }

    /// Send a request without a body, signed with the empty-payload hash.
    async fn send_empty(
        &self,
        method: reqwest::Method,
        key: &str,
        query: &[(String, String)],
        extra_headers: &[(String, String)],
    ) -> Result<reqwest::Response, S3Error> {
        self.send(method, key, query, extra_headers, EMPTY_PAYLOAD, None)
            .await
    }

    /// `HEAD` an object: its size, entity tag and modification time.
    pub async fn head(&self, key: &str) -> Result<ObjectMeta, S3Error> {
        let res = self
            .send_empty(reqwest::Method::HEAD, key, &[], &[])
            .await?;
        let res = found(res, key).await?;
        Ok(ObjectMeta {
            // A `HEAD` response has no body, so the size of the object is only in the
            // header: the length of the body would be zero.
            size: content_length(&res).unwrap_or(0),
            etag: etag_of(&res),
            last_modified: last_modified_of(&res),
        })
    }

    /// `GET` an object, optionally a byte range of it.
    ///
    /// The body is streamed: nothing is buffered in memory.
    pub async fn get(&self, key: &str, range: Option<(u64, u64)>) -> Result<ObjectStream, S3Error> {
        let mut extra = Vec::new();
        if let Some((start, end)) = range {
            extra.push(("range".to_string(), format!("bytes={start}-{end}")));
        }
        let res = self
            .send_empty(reqwest::Method::GET, key, &[], &extra)
            .await?;
        let res = found(res, key).await?;
        Ok(ObjectStream {
            status: res.status().as_u16(),
            etag: etag_of(&res),
            last_modified: last_modified_of(&res),
            content_length: res.content_length().or_else(|| content_length(&res)),
            content_range: header_str(&res, reqwest::header::CONTENT_RANGE).map(str::to_string),
            body: res,
        })
    }

    /// `GET` a whole object into memory, for a caller that needs all of its bytes at once.
    ///
    /// The answer is given up as stalled after [`STALL_TIMEOUT`], and the body gets the
    /// same budget plus the time its length takes at [`MIN_UPLOAD_RATE`], like an upload.
    pub async fn get_bytes(&self, key: &str) -> Result<Vec<u8>, S3Error> {
        let object = tokio::time::timeout(STALL_TIMEOUT, self.get(key, None))
            .await
            .map_err(|_| S3Error::stalled(STALL_TIMEOUT))??;
        let budget = STALL_TIMEOUT
            + Duration::from_secs(object.content_length.unwrap_or(0) / MIN_UPLOAD_RATE);
        let bytes = tokio::time::timeout(budget, object.body.bytes())
            .await
            .map_err(|_| S3Error::stalled(budget))?
            .map_err(S3Error::transport)?;
        Ok(bytes.to_vec())
    }

    /// One page of `ListObjectsV2`.
    ///
    /// With a `delimiter`, the keys directly under `prefix` come back as objects and
    /// the deeper ones as common prefixes (the "directories").
    pub async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        continuation: Option<&str>,
    ) -> Result<ListResult, S3Error> {
        let mut query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("max-keys".to_string(), MAX_KEYS.to_string()),
        ];
        if !prefix.is_empty() {
            query.push(("prefix".to_string(), prefix.to_string()));
        }
        if let Some(delimiter) = delimiter {
            query.push(("delimiter".to_string(), delimiter.to_string()));
        }
        if let Some(token) = continuation {
            query.push(("continuation-token".to_string(), token.to_string()));
        }

        let res = self
            .send_empty(reqwest::Method::GET, "", &query, &[])
            .await?;
        let res = successful(res).await?;
        let xml = res.text().await.map_err(S3Error::transport)?;
        Ok(parse_list_result(&xml))
    }

    /// Every page of `ListObjectsV2`, merged: the objects and the common prefixes of
    /// the whole listing, in the order the service returned them.
    pub async fn list_all(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<ListResult, S3Error> {
        let mut all = ListResult::default();
        let mut continuation: Option<String> = None;
        loop {
            // A stalled page is an error rather than a hang.
            let page = tokio::time::timeout(
                STALL_TIMEOUT,
                self.list(prefix, delimiter, continuation.as_deref()),
            )
            .await
            .map_err(|_| S3Error::stalled(STALL_TIMEOUT))??;
            all.objects.extend(page.objects);
            all.prefixes.extend(page.prefixes);
            continuation = page.next_continuation;
            if continuation.is_none() {
                return Ok(all);
            }
        }
    }

    /// `PUT` an object: upload the bytes of `body` under `key`.
    ///
    /// `content_type` and `cache_control` are stored with the object; the second one
    /// is what a CDN in front of the bucket takes its time to live from.
    pub async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        cache_control: Option<&str>,
    ) -> Result<ObjectMeta, S3Error> {
        let hash = hex(&Sha256::digest(&body));
        let extra = object_headers(content_type, cache_control);
        let res = self
            .send(
                reqwest::Method::PUT,
                key,
                &[],
                &extra,
                &hash,
                Some(reqwest::Body::from(body)),
            )
            .await?;
        let res = successful(res).await?;
        Ok(ObjectMeta {
            size: 0,
            etag: etag_of(&res),
            last_modified: None,
        })
    }

    /// `PUT` a file as an object.
    ///
    /// The file is read twice — once to hash it (the signature covers the body) and
    /// once to send it — so a large file never sits in memory.
    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        content_type: Option<&str>,
        cache_control: Option<&str>,
    ) -> Result<ObjectMeta, S3Error> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| S3Error::file(path, e))?;
        let hash = hash_file(path).await?;

        let mut extra = object_headers(content_type, cache_control);
        // The length is known, so it is sent as a header: a chunked upload is refused
        // by the services that expect a signed body.
        extra.push(("content-length".to_string(), meta.len().to_string()));

        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| S3Error::file(path, e))?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));

        // A stalled upload must not hang the publish; the budget grows with the file.
        let budget = STALL_TIMEOUT + Duration::from_secs(meta.len() / MIN_UPLOAD_RATE);
        let res = tokio::time::timeout(budget, async {
            let res = self
                .send(reqwest::Method::PUT, key, &[], &extra, &hash, Some(body))
                .await?;
            successful(res).await
        })
        .await
        .map_err(|_| S3Error::stalled(budget))??;
        Ok(ObjectMeta {
            size: meta.len(),
            etag: etag_of(&res),
            last_modified: None,
        })
    }

    /// `DELETE` an object: remove it from the bucket.
    ///
    /// A missing object is not an error: the bucket ends up in the wanted state either
    /// way.
    pub async fn delete(&self, key: &str) -> Result<(), S3Error> {
        let res = tokio::time::timeout(
            STALL_TIMEOUT,
            self.send_empty(reqwest::Method::DELETE, key, &[], &[]),
        )
        .await
        .map_err(|_| S3Error::stalled(STALL_TIMEOUT))??;
        if res.status().as_u16() == 404 {
            return Ok(());
        }
        successful(res).await.map(drop)
    }
}

/// The headers an object is stored with.
fn object_headers(
    content_type: Option<&str>,
    cache_control: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(content_type) = content_type {
        headers.push(("content-type".to_string(), content_type.to_string()));
    }
    if let Some(cache_control) = cache_control {
        headers.push(("cache-control".to_string(), cache_control.to_string()));
    }
    headers
}

/// The response when the service answered with a success, its error otherwise.
async fn successful(res: reqwest::Response) -> Result<reqwest::Response, S3Error> {
    if res.status().is_success() {
        Ok(res)
    } else {
        Err(error_from_response(res).await)
    }
}

/// The same, with a `404` reported as the object `key` being missing.
async fn found(res: reqwest::Response, key: &str) -> Result<reqwest::Response, S3Error> {
    if res.status().as_u16() == 404 {
        return Err(S3Error::not_found(key));
    }
    successful(res).await
}

/// The SHA-256 of a file, in hex: the payload hash the signature carries.
async fn hash_file(path: &Path) -> Result<String, S3Error> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| S3Error::file(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|e| S3Error::file(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

async fn error_from_response(res: reqwest::Response) -> S3Error {
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    let message = xml_text(&body, "Message")
        .or_else(|| xml_text(&body, "message"))
        .unwrap_or_else(|| body.chars().take(200).collect());
    S3Error {
        status,
        message,
        local: false,
    }
}

/// Percent-encode an object key for a public URL (path segments keep their slashes).
pub fn encode_path(key: &str) -> String {
    uri_encode(key, false)
}

/// A response header as text, when it is there and is valid text.
fn header_str(res: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<&str> {
    res.headers().get(name).and_then(|v| v.to_str().ok())
}

/// The entity tag of a response, without the surrounding quotes ("" when absent).
fn etag_of(res: &reqwest::Response) -> String {
    header_str(res, reqwest::header::ETAG)
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default()
}

/// The `Last-Modified` of a response, as a UTC timestamp.
fn last_modified_of(res: &reqwest::Response) -> Option<DateTime<Utc>> {
    header_str(res, reqwest::header::LAST_MODIFIED).and_then(parse_http_date)
}

/// The length of a response body, from the `Content-Length` header.
///
/// A `HEAD` response carries no body: the size of the object is only in the header.
fn content_length(res: &reqwest::Response) -> Option<u64> {
    header_str(res, reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok())
}

/// Turn an HTTP date into a UTC timestamp.
fn parse_http_date(value: &str) -> Option<DateTime<Utc>> {
    let time = httpdate::parse_http_date(value).ok()?;
    let secs = time.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    DateTime::from_timestamp(secs, 0)
}

/// The signed form of a request.
#[derive(Debug, Clone)]
struct SignedRequest {
    /// The `Authorization` header value.
    authorization: String,
    /// The `x-amz-date` header value.
    amz_date: String,
    /// The canonical query string that was signed.
    canonical_query: String,
    /// The SHA-256 of the canonical request; only the tests read it.
    #[cfg(test)]
    canonical_request_hash: String,
}

/// The signing material of a request.
struct Signer<'a> {
    secret: &'a str,
    access_key_id: &'a str,
    region: &'a str,
}

impl Signer<'_> {
    /// Sign a request with AWS Signature Version 4.
    ///
    /// `headers` are the headers to sign: their names are lowercased, their values
    /// trimmed and the whole list sorted by name. `timestamp` is the moment of the
    /// request, so the signature is reproducible in a test.
    fn sign(
        &self,
        method: &str,
        canonical_path: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        payload_hash: &str,
        timestamp: DateTime<Utc>,
    ) -> SignedRequest {
        let amz_date = timestamp.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = timestamp.format("%Y%m%d").to_string();

        let mut headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        headers.push(("x-amz-date".to_string(), amz_date.clone()));
        headers.sort_by(|a, b| a.0.cmp(&b.0));

        let mut canonical_headers = String::new();
        let mut signed_names = Vec::new();
        for (name, value) in &headers {
            canonical_headers.push_str(name);
            canonical_headers.push(':');
            canonical_headers.push_str(value);
            canonical_headers.push('\n');
            signed_names.push(name.as_str());
        }
        let signed_headers = signed_names.join(";");

        let canonical_query = canonical_query_string(query);
        let canonical_request = format!(
            "{method}\n{canonical_path}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let canonical_request_hash = hex(&Sha256::digest(canonical_request.as_bytes()));

        let scope = format!("{date_stamp}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!("{ALGORITHM}\n{amz_date}\n{scope}\n{canonical_request_hash}");

        let key = signing_key(self.secret, &date_stamp, self.region);
        let signature = hex(&hmac_sha256(&key, string_to_sign.as_bytes()));

        let authorization = format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key_id
        );

        SignedRequest {
            authorization,
            amz_date,
            canonical_query,
            #[cfg(test)]
            canonical_request_hash,
        }
    }
}

/// `HMAC-SHA256(key, data)`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// The SigV4 signing key: `HMAC` chained over date, region, service and terminator.
fn signing_key(secret: &str, date_stamp: &str, region: &str) -> Vec<u8> {
    let mut key = format!("AWS4{secret}").into_bytes();
    for part in [date_stamp, region, "s3", "aws4_request"] {
        key = hmac_sha256(&key, part.as_bytes());
    }
    key
}

/// Percent-encode a path segment or query value the way SigV4 expects.
///
/// Only unreserved characters survive; `/` is kept in path segments and encoded in
/// query values.
fn uri_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            b'/' if !encode_slash => out.push('/'),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// The canonical query string: sorted by name then value, every part encoded.
fn canonical_query_string(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The text inside the first `<tag>…</tag>` of an XML document.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml_decode(&xml[start..end]))
}

/// Every `<tag>…</tag>` block of an XML document.
fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = start + open.len();
        let Some(len) = rest[after..].find(&close) else {
            break;
        };
        blocks.push(rest[after..after + len].to_string());
        rest = &rest[after + len + close.len()..];
    }
    blocks
}

/// Decode the XML entities S3 escapes in keys.
fn xml_decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = entity
                    .strip_prefix('#')
                    .and_then(|n| n.strip_prefix('x').or(Some(n)))
                    .and_then(|n| {
                        u32::from_str_radix(n, if entity.starts_with("#x") { 16 } else { 10 }).ok()
                    });
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(&rest[..=end]),
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Parse a `ListObjectsV2` response.
fn parse_list_result(xml: &str) -> ListResult {
    let mut result = ListResult::default();
    for block in xml_blocks(xml, "Contents") {
        let Some(key) = xml_text(&block, "Key") else {
            continue;
        };
        let size = xml_text(&block, "Size")
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let etag = xml_text(&block, "ETag")
            .map(|e| e.trim().trim_matches('"').to_string())
            .unwrap_or_default();
        let last_modified = xml_text(&block, "LastModified").and_then(|value| {
            DateTime::parse_from_rfc3339(value.trim())
                .ok()
                .map(|t| t.with_timezone(&Utc))
        });
        result.objects.push(ListedObject {
            key,
            size,
            etag,
            last_modified,
        });
    }
    for block in xml_blocks(xml, "CommonPrefixes") {
        if let Some(prefix) = xml_text(&block, "Prefix") {
            result.prefixes.push(prefix);
        }
    }
    result.next_continuation = xml_text(xml, "NextContinuationToken")
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_string());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_encoding_follows_the_sigv4_rules() {
        assert_eq!(uri_encode("a/b c", false), "a/b%20c");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("~-_.9AZaz", true), "~-_.9AZaz");
        assert_eq!(uri_encode("a+b=c&d", true), "a%2Bb%3Dc%26d");
    }

    #[test]
    fn query_strings_are_sorted_and_encoded() {
        let query = vec![
            ("prefix".to_string(), "a b/".to_string()),
            ("list-type".to_string(), "2".to_string()),
        ];
        assert_eq!(
            canonical_query_string(&query),
            "list-type=2&prefix=a%20b%2F"
        );
    }

    #[test]
    fn signing_key_matches_the_aws_examples() {
        // The key derivation of the AWS documentation, cross-checked with botocore.
        assert_eq!(
            hex(&signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20130524",
                "us-east-1"
            )),
            "f117494eff5d09da21cbf7f0339559ea04fc9582d31299cb992be70a6b27c97a"
        );
        assert_eq!(
            hex(&signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20150830",
                "us-east-1"
            )),
            "32f78051dcde24c552811d654f4a769112bb834b03975cdd6b1fd7d16248c269"
        );
    }

    #[test]
    fn signature_matches_the_aws_get_object_example() {
        // The "GET Object" example of the AWS Signature Version 4 documentation:
        // a ranged read of `test.txt` with the example credentials. The canonical
        // request hash is the one the documentation prints, and the signature was
        // confirmed with botocore 1.42.
        let timestamp = DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let signed = Signer {
            secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            access_key_id: "AKIAIOSFODNN7EXAMPLE",
            region: "us-east-1",
        }
        .sign(
            "GET",
            "/test.txt",
            &[],
            &[
                (
                    "host".to_string(),
                    "examplebucket.s3.amazonaws.com".to_string(),
                ),
                ("range".to_string(), "bytes=0-9".to_string()),
                (
                    "x-amz-content-sha256".to_string(),
                    EMPTY_PAYLOAD.to_string(),
                ),
            ],
            EMPTY_PAYLOAD,
            timestamp,
        );
        assert_eq!(
            signed.canonical_request_hash,
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=67fe34c8530db585abddc51067328adfedb6e42487d2566dc7d927d6e2722900"
        );
    }

    #[test]
    fn query_strings_are_signed_in_the_canonical_form() {
        let timestamp = DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let signed = Signer {
            secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            access_key_id: "AKIAIOSFODNN7EXAMPLE",
            region: "us-east-1",
        }
        .sign(
            "GET",
            "/bucket",
            &[
                ("list-type".to_string(), "2".to_string()),
                ("prefix".to_string(), "textures/".to_string()),
            ],
            &[("host".to_string(), "example.com".to_string())],
            EMPTY_PAYLOAD,
            timestamp,
        );
        assert_eq!(signed.canonical_query, "list-type=2&prefix=textures%2F");
    }

    #[test]
    fn list_results_are_parsed() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>assets</Name>
  <Prefix>textures/</Prefix>
  <KeyCount>2</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>textures/ui/a.png</Key>
    <LastModified>2026-10-20T08:35:02.000Z</LastModified>
    <ETag>&quot;abc123&quot;</ETag>
    <Size>1234</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <Contents>
    <Key>textures/ui/b&amp;c.png</Key>
    <LastModified>2026-10-21T08:35:02.000Z</LastModified>
    <ETag>&quot;def456&quot;</ETag>
    <Size>42</Size>
  </Contents>
  <CommonPrefixes><Prefix>textures/ui/sub/</Prefix></CommonPrefixes>
</ListBucketResult>"#;
        let result = parse_list_result(xml);
        assert_eq!(result.objects.len(), 2);
        assert_eq!(result.objects[0].key, "textures/ui/a.png");
        assert_eq!(result.objects[0].size, 1234);
        assert_eq!(result.objects[0].etag, "abc123");
        assert_eq!(result.objects[1].key, "textures/ui/b&c.png");
        assert_eq!(result.objects[1].size, 42);
        assert_eq!(result.prefixes, vec!["textures/ui/sub/"]);
        assert!(result.next_continuation.is_none());
    }

    #[test]
    fn truncated_listings_keep_the_continuation_token() {
        let xml = r#"<ListBucketResult>
          <IsTruncated>true</IsTruncated>
          <NextContinuationToken>tok/en+1</NextContinuationToken>
        </ListBucketResult>"#;
        let result = parse_list_result(xml);
        assert_eq!(result.next_continuation.as_deref(), Some("tok/en+1"));
    }
}
