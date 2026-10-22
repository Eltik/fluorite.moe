//! Object storage: the client, the settings, the providers and the publisher.
//!
//! One S3-compatible layer serves both halves of the project: the API reads the roots
//! that live in the bucket, and the unpacker writes the extraction output into it.
//! The module is in four parts:
//!
//! - [`client`]: the requests, the signature and the object operations.
//! - [`settings`]: the `S3_*` and `CLOUDFLARE_*` variables, read the same way by both halves.
//! - [`provider`]: the table of the known services and what they need.
//! - [`publish`]: copying the local roots into the bucket.

mod client;
mod provider;
mod publish;
mod settings;

pub use client::{
    Credentials, ListResult, ListedObject, ObjectMeta, ObjectStream, S3Client, S3Error, encode_path,
};
pub use provider::Provider;
pub use publish::{PublishOptions, PublishReport, publish};
pub use settings::{S3Settings, flag, flag_opt};
