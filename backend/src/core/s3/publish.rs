//! Publishing the extraction output into an S3-compatible bucket.
//!
//! The unpacker writes the assets to the local disk; this copies the roots that live in
//! the bucket into it, so an update reaches the bucket on its own — no second tool, no
//! manual step — and the local copy can go away right after the upload.
//!
//! What is uploaded is decided by comparing the local files with the objects already in
//! the bucket: a file is uploaded when the bucket has no object for it, when the sizes
//! differ, or when the local file is more than a second newer than the object. The last
//! rule needs the clocks of the two machines to agree (they do, both are on NTP); the
//! size catches a change that keeps the clock from telling.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use md5::{Digest, Md5};

use super::client::{S3Client, S3Error};
use super::settings::S3Settings;
use crate::core::roots::Root;

/// How much newer a local file must be than the object in the bucket to be uploaded.
///
/// The bucket stores the time of the upload with a one-second resolution, so a file
/// that was just uploaded looks up to a second older than it is: without the margin it
/// would be uploaded again by every run.
const NEWER_MARGIN: Duration = Duration::from_secs(1);

/// How many errors are kept for the log.
const MAX_ERRORS: usize = 20;

/// How many uploads a root completes between two progress lines.
const PROGRESS_EVERY: usize = 1000;

/// How long to wait before each new attempt of a call that failed for a passing reason.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// The chunk a local file is read in to hash it.
const HASH_CHUNK: usize = 1 << 20;

/// What a publish run did.
#[derive(Debug, Default)]
pub struct PublishReport {
    /// Files uploaded.
    pub uploaded: u64,
    /// Bytes uploaded.
    pub bytes: u64,
    /// Files that were already in the bucket.
    pub skipped: u64,
    /// Local files removed once they were in the bucket, including the ones that were
    /// already there.
    pub removed: u64,
    /// Objects removed from the bucket (the prune).
    pub pruned: u64,
    /// Files that could not be uploaded.
    pub failed: u64,
    /// The first errors, for the log.
    pub errors: Vec<String>,
}

impl PublishReport {
    /// A one-line summary, for the log.
    pub fn summary(&self) -> String {
        format!(
            "uploaded {} ({} MiB), skipped {}, removed {}, pruned {}, failed {}",
            self.uploaded,
            self.bytes as f64 / (1024.0 * 1024.0),
            self.skipped,
            self.removed,
            self.pruned,
            self.failed
        )
    }

    /// Count a failed object; its error is kept while there is room in the log.
    fn fail(&mut self, key: &str, error: &S3Error) {
        self.failed += 1;
        self.note(format!("{key}: {error}"));
    }

    /// Count the removal of a local file; a file that could not be removed is logged.
    fn removal(&mut self, path: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.removed += 1,
            Err(error) => self.note(format!("{}: {error}", path.display())),
        }
    }

    /// Keep an error for the log while there is room in it.
    fn note(&mut self, error: String) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(error);
        }
    }
}

/// How a publish run behaves.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    /// Remove the local copy of a file once it is in the bucket, including the files
    /// that were already there.
    pub remove_local: bool,
    /// Delete the objects that are gone locally.
    pub prune: bool,
    /// Report what would change, upload nothing.
    pub dry_run: bool,
}

/// A local file that is a candidate for the bucket.
struct LocalFile {
    path: PathBuf,
    /// The path below the root, with `/` as the separator.
    rel: String,
    size: u64,
    mtime: SystemTime,
}

/// An object the bucket holds.
struct RemoteObject {
    size: u64,
    /// The time it was stored.
    modified: Option<DateTime<Utc>>,
    /// The entity tag: the MD5 of the bytes, for an object stored with a single upload.
    etag: String,
}

/// What the bucket holds under a prefix, by key.
type RemoteObjects = HashMap<String, RemoteObject>;

/// Publish the roots that live in the bucket.
///
/// The roots are the ones in `S3_ROOTS`: the local directory of a root is the
/// one the API reads (`Root::rel_dir`), and its objects live under the same name in the
/// bucket, so the two sides agree on the keys.
pub async fn publish(
    settings: &S3Settings,
    output: &Path,
    options: &PublishOptions,
) -> Result<PublishReport, String> {
    // The prune deletes what is not on the disk, and the removal of the local copies is
    // what takes files off it: together they would empty the bucket.
    if options.prune && options.remove_local {
        return Err(
            "--prune needs the complete local tree; it cannot run while local \
             copies are removed (--remove-local or S3_KEEP_LOCAL=0)"
                .to_string(),
        );
    }
    let client = settings.client()?;
    let mut report = PublishReport::default();
    for root in &settings.roots {
        let dir = root.dir(output);
        if !dir.is_dir() {
            report
                .errors
                .push(format!("{}: no local directory", dir.display()));
            continue;
        }
        publish_root(&client, settings, *root, &dir, options, &mut report).await;
    }
    Ok(report)
}

/// Publish one root.
async fn publish_root(
    client: &S3Client,
    settings: &S3Settings,
    root: Root,
    dir: &Path,
    options: &PublishOptions,
    report: &mut PublishReport,
) {
    let prefix = settings.root_prefix(root);
    let files = match walk(dir).await {
        Ok(files) => files,
        Err(error) => {
            report.failed += 1;
            report.errors.push(format!("{}: {error}", dir.display()));
            return;
        }
    };
    // A root that is empty on the disk has nothing to upload: the bucket is not even
    // listed. That is the normal case once the local copies were removed.
    if files.is_empty() {
        return;
    }
    let remote = match list_objects(client, &prefix).await {
        Ok(remote) => remote,
        Err(error) => {
            report.failed += 1;
            report.errors.push(format!("{}: {error}", root.name()));
            return;
        }
    };

    let removed_before = report.removed;
    let mut local_keys: HashSet<String> = HashSet::with_capacity(files.len());
    let mut stale = Vec::new();
    let mut held = Vec::new();
    for file in &files {
        let key = format!("{prefix}{}", file.rel);
        local_keys.insert(key.clone());
        let object = remote.get(&key);
        if !is_stale(file, object) {
            match object {
                // The bucket already has it, so the local copy can go as well.
                Some(object) if options.remove_local && !options.dry_run => {
                    held.push((key, file, object.etag.as_str()));
                }
                _ => report.skipped += 1,
            }
            continue;
        }
        if options.dry_run {
            report.uploaded += 1;
            report.bytes += file.size;
            continue;
        }
        stale.push((key, file));
    }

    // A file the bucket already has only goes once its hash shows the object holds the
    // same bytes: one that differs is uploaded after all, one that cannot be told stays.
    let mut checks = stream::iter(held)
        .map(|(key, file, etag)| async move {
            let same = same_content(&file.path, etag).await;
            (key, file, same)
        })
        .buffer_unordered(settings.upload_concurrency);
    while let Some((key, file, same)) = checks.next().await {
        match same {
            Ok(Some(true)) => {
                report.skipped += 1;
                let removed = tokio::fs::remove_file(&file.path).await;
                report.removal(&file.path, removed);
            }
            Ok(Some(false)) => stale.push((key, file)),
            Ok(None) => report.skipped += 1,
            Err(error) => {
                report.skipped += 1;
                report.note(format!("{}: {error}", file.path.display()));
            }
        }
    }

    // The uploads run side by side; their results are counted here as they come in.
    let total = stale.len();
    let mut uploads = stream::iter(stale)
        .map(|(key, file)| async move {
            let uploaded = retry(|| {
                client.put_file(
                    &key,
                    &file.path,
                    content_type(&file.rel),
                    settings.cache_control.as_deref(),
                )
            })
            .await;
            let removed = match uploaded {
                Ok(_) if options.remove_local => Some(tokio::fs::remove_file(&file.path).await),
                _ => None,
            };
            (key, file, uploaded, removed)
        })
        .buffer_unordered(settings.upload_concurrency);
    let mut done = 0;
    while let Some((key, file, uploaded, removed)) = uploads.next().await {
        done += 1;
        match uploaded {
            Ok(_) => {
                report.uploaded += 1;
                report.bytes += file.size;
            }
            Err(error) => report.fail(&key, &error),
        }
        if let Some(removed) = removed {
            report.removal(&file.path, removed);
        }
        if done % PROGRESS_EVERY == 0 || done == total {
            println!("  publish {}: {done}/{total} uploaded", root.name());
        }
    }

    if options.prune && !options.dry_run {
        prune(
            client,
            &remote,
            &local_keys,
            settings.upload_concurrency,
            report,
        )
        .await;
    }
    if options.remove_local && report.removed > removed_before {
        remove_empty_dirs(dir).await;
    }
}

/// Whether a local file must go into the bucket: the bucket has no object for it, the
/// sizes differ, or the file is newer than the object by more than [`NEWER_MARGIN`].
fn is_stale(file: &LocalFile, object: Option<&RemoteObject>) -> bool {
    let Some(object) = object else {
        return true;
    };
    let stored = object
        .modified
        .map(SystemTime::from)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    object.size != file.size || file.mtime > stored + NEWER_MARGIN
}

/// Whether a local file holds the bytes of an object, by the MD5 that is the entity tag
/// of an object stored with a single upload. `None` when the tag is not an MD5: an
/// object uploaded in parts is tagged `<hash of the parts>-<count>`.
async fn same_content(path: &Path, etag: &str) -> std::io::Result<Option<bool>> {
    let etag = etag.trim().trim_matches('"').to_ascii_lowercase();
    if etag.contains('-') || etag.len() != 32 {
        return Ok(None);
    }
    Ok(Some(md5_file(path).await? == etag))
}

/// The MD5 of a file, in lowercase hex, read in chunks.
async fn md5_file(path: &Path) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Md5::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Make a call to the bucket, and make it again after [`RETRY_DELAYS`] while it fails
/// for a passing reason (the service is busy or failing, the answer never came).
///
/// An upload opens its file again on every attempt, so a retry sends the whole file.
async fn retry<T, F, Fut>(mut call: F) -> Result<T, S3Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, S3Error>>,
{
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match call().await {
            Err(error) if error.is_transient() => match delays.next() {
                Some(delay) => tokio::time::sleep(*delay).await,
                None => return Err(error),
            },
            result => return result,
        }
    }
}

/// Delete the objects that are not on the disk any more, `concurrency` at once.
async fn prune(
    client: &S3Client,
    remote: &RemoteObjects,
    local_keys: &HashSet<String>,
    concurrency: usize,
    report: &mut PublishReport,
) {
    let mut deletes = stream::iter(remote.keys().filter(|key| !local_keys.contains(*key)))
        .map(|key| async move { (key, retry(|| client.delete(key)).await) })
        .buffer_unordered(concurrency);
    while let Some((key, deleted)) = deletes.next().await {
        match deleted {
            Ok(()) => report.pruned += 1,
            Err(error) => report.fail(key, &error),
        }
    }
}

/// Every object under `prefix`, with its size, the time it was stored and its tag.
async fn list_objects(client: &S3Client, prefix: &str) -> Result<RemoteObjects, S3Error> {
    let listing = client.list_all(prefix, None).await?;
    Ok(listing
        .objects
        .into_iter()
        .map(|object| {
            let remote = RemoteObject {
                size: object.size,
                modified: object.last_modified,
                etag: object.etag,
            };
            (object.key, remote)
        })
        .collect())
}

/// Whether a local name is left out of the bucket: hidden files, and the temporary and
/// intermediate files of the extraction.
fn is_excluded(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(".tmp") || name.ends_with(".mpk")
}

/// Every file below `dir`, with its size and the time it was written.
async fn walk(dir: &Path) -> std::io::Result<Vec<LocalFile>> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, prefix)) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(entries) => entries,
            // A directory that vanished under the walk is not an error.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_excluded(name) {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push((path, format!("{prefix}{name}/")));
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            out.push(LocalFile {
                path,
                rel: format!("{prefix}{name}"),
                size: meta.len(),
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(out)
}

/// Remove the directories that the removal of the local copies emptied.
async fn remove_empty_dirs(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        let mut children = Vec::new();
        let mut empty = true;
        while let Ok(Some(entry)) = entries.next_entry().await {
            empty = false;
            if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
                children.push(entry.path());
            }
        }
        if empty && current != dir {
            let _ = tokio::fs::remove_dir(&current).await;
        }
        stack.extend(children);
    }
}

/// The content type an object is stored with, from the name of the file.
///
/// The API serves the objects with the same types, so a file that is uploaded and a
/// file that is served look the same to a browser.
pub fn content_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ktx2" => "image/ktx2",
        "dds" => "image/vnd-ms.dds",
        "tga" => "image/x-tga",
        "psd" => "image/vnd.adobe.photoshop",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "json" | "jsonl" => "application/json",
        "txt" | "lua" | "xml" | "csv" => "text/plain",
        "html" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    })
}
