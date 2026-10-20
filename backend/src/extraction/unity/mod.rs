//! Unity SerializedFile parsing, adapted from Myrtle's unpacker.
pub mod endian_reader;
pub mod object;
pub mod object_reader;
pub mod serialized_file;
pub mod type_tree;

pub use object::value_bytes;
pub use serialized_file::{ObjectInfo, SerializedFile};
