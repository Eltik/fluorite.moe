//! Rigged-model export: Unity prefabs (Transform hierarchy, skinned meshes, materials) → GLB.
//!
//! - [`anim`]: AnimationClips (ACL-compressed) → glTF animations; [`acl`] wraps the vendored
//!   ACL decompressor (`--features acl`)
//! - [`clips`]: one animation-only GLB per clip, bound by node name to an exported model
//! - [`humanoid`]: Unity Humanoid muscle curves → bone rotations through the Avatar
//! - [`chunk`]: load a `.chk` as named SerializedFiles (CAB names resolve cross-file PPtrs)
//! - [`store`]: keep every chunk's SerializedFiles and mesh streams on disk after one download
//! - [`export`]: walk a prefab's Transforms and renderers into a glTF document
//! - [`material`]: Unity materials/textures → glTF materials with shader data in `extras`
//! - [`skin`]: bone weights/indices and bind poses
//! - [`convert`]: left-handed Unity → right-handed glTF math
//! - [`gltf`]: document builder, GLB container and structural checks
pub mod acl;
pub mod anim;
pub mod avatar;
pub mod chunk;
pub mod clips;
pub mod convert;
pub mod entity;
pub mod export;
pub mod gltf;
pub mod humanoid;
pub mod inspect;
pub mod material;
pub mod skin;
pub mod store;

pub use chunk::{ChunkFiles, ContainerEntry, NamedFile, containers, load_chunk, load_source_dir};
pub use export::{EntityExport, ExportOptions, ExportStats, Exporter, ObjRef};
