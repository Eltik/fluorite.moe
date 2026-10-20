# unity-asset-binary 0.2.0 (vendored)

Upstream: https://github.com/Latias94/unity-asset (MIT). Wired in via `[patch.crates-io]`
in `backend/Cargo.toml`.

Local change: `src/typetree/serializer.rs` rejected every array and `TypelessData` longer
than 1,000,000 elements, so the lenient parser silently dropped `m_VertexData` (and every
field after it) from high-poly meshes. Lengths above the cap are now accepted when they fit
in the object's remaining bytes.
