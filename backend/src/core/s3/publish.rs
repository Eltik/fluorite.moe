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

/// What a publish run did.
#[derive(Debug, Default)]
pub struct PublishReport {
    /// Files uploaded.
    pub uploaded: u64,
    /// Bytes uploaded.
    pub bytes: u64,
    /// Files that were already in the bucket.
    pub skipped: u64,
    /// Local files removed after the upload.
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
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("{key}: {error}"));
        }
    }
}

/// How a publish run behaves.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    /// Remove the local copy of a file once it is in the bucket.
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

/// What the bucket holds under a prefix: the size and the storage time of every object,
/// by key.
type RemoteObjects = HashMap<String, (u64, Option<DateTime<Utc>>)>;

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

    let mut local_keys: HashSet<String> = HashSet::with_capacity(files.len());
    for file in &files {
        let key = format!("{prefix}{}", file.rel);
        local_keys.insert(key.clone());
        if !is_stale(file, remote.get(&key)) {
            report.skipped += 1;
            continue;
        }
        if options.dry_run {
            report.uploaded += 1;
            report.bytes += file.size;
            continue;
        }
        let uploaded = client
            .put_file(
                &key,
                &file.path,
                content_type(&file.rel),
                settings.cache_control.as_deref(),
            )
            .await;
        if let Err(error) = uploaded {
            report.fail(&key, &error);
            continue;
        }
        report.uploaded += 1;
        report.bytes += file.size;
        if options.remove_local {
            match tokio::fs::remove_file(&file.path).await {
                Ok(()) => report.removed += 1,
                Err(error) => report
                    .errors
                    .push(format!("{}: {error}", file.path.display())),
            }
        }
    }

    if options.prune && !options.dry_run {
        prune(client, &remote, &local_keys, report).await;
    }
    if options.remove_local && report.removed > 0 {
        remove_empty_dirs(dir).await;
    }
}

/// Whether a local file must go into the bucket: the bucket has no object for it, the
/// sizes differ, or the file is newer than the object by more than [`NEWER_MARGIN`].
fn is_stale(file: &LocalFile, object: Option<&(u64, Option<DateTime<Utc>>)>) -> bool {
    let Some((size, modified)) = object else {
        return true;
    };
    let stored = modified
        .map(SystemTime::from)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    *size != file.size || file.mtime > stored + NEWER_MARGIN
}

/// Delete the objects that are not on the disk any more.
async fn prune(
    client: &S3Client,
    remote: &RemoteObjects,
    local_keys: &HashSet<String>,
    report: &mut PublishReport,
) {
    for key in remote.keys() {
        if local_keys.contains(key) {
            continue;
        }
        match client.delete(key).await {
            Ok(()) => report.pruned += 1,
            Err(error) => report.fail(key, &error),
        }
    }
}

/// Every object under `prefix`, with its size and the time it was stored.
async fn list_objects(client: &S3Client, prefix: &str) -> Result<RemoteObjects, S3Error> {
    let listing = client.list_all(prefix, None).await?;
    Ok(listing
        .objects
        .into_iter()
        .map(|object| (object.key, (object.size, object.last_modified)))
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

/// Remove the directories that the upload emptied.
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
