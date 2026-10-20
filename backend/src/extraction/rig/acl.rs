//! Safe wrapper over the C++ shim around ACL's decompressor (`--features acl`).
//!
//! Endfield stores AnimationClip curves as stock ACL v2.1.0 `compressed_tracks` buffers
//! (`m_AclCompressedBuffer.TransformBufferData` / `FloatBufferData` / `RootMotionBufferData`).

/// Header facts of a compressed buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AclInfo {
    pub num_tracks: u32,
    pub num_samples: u32,
    pub sample_rate: f32,
    /// ACL `track_type8`: 0 = float1f, 12 = qvvf (transforms).
    pub track_type: u8,
}

/// Floats written per transform track and sample: rotation xyzw, translation xyz, scale xyz.
pub const TRANSFORM_STRIDE: usize = 10;

#[cfg(feature = "acl")]
mod ffi {
    unsafe extern "C" {
        pub fn fluorite_acl_info(
            data: *const u8,
            len: usize,
            num_tracks: *mut u32,
            num_samples: *mut u32,
            sample_rate: *mut f32,
            track_type: *mut u8,
        ) -> i32;
        pub fn fluorite_acl_transforms(
            data: *const u8,
            len: usize,
            out: *mut f32,
            out_len: usize,
        ) -> i32;
        pub fn fluorite_acl_floats(
            data: *const u8,
            len: usize,
            out: *mut f32,
            out_len: usize,
        ) -> i32;
    }
}

/// Whether this build can decompress ACL buffers.
#[must_use]
pub fn available() -> bool {
    cfg!(feature = "acl")
}

/// Validate a buffer (including ACL's hash) and read its header.
#[cfg(feature = "acl")]
pub fn info(data: &[u8]) -> Result<AclInfo, String> {
    let (mut num_tracks, mut num_samples, mut sample_rate, mut track_type) =
        (0u32, 0u32, 0f32, 0u8);
    // SAFETY: the shim only reads `len` bytes of `data` (copying them to aligned memory) and
    // writes the four out-parameters, which point at live locals.
    let rc = unsafe {
        ffi::fluorite_acl_info(
            data.as_ptr(),
            data.len(),
            &mut num_tracks,
            &mut num_samples,
            &mut sample_rate,
            &mut track_type,
        )
    };
    if rc != 0 {
        return Err(format!("invalid ACL buffer ({rc})"));
    }
    Ok(AclInfo {
        num_tracks,
        num_samples,
        sample_rate,
        track_type,
    })
}

/// Every sample of a transform buffer: `out[(sample * tracks + track) * TRANSFORM_STRIDE + k]`.
#[cfg(feature = "acl")]
pub fn transforms(data: &[u8]) -> Result<(AclInfo, Vec<f32>), String> {
    let info = info(data)?;
    if info.track_type != 12 {
        return Err(format!(
            "not a transform buffer (track type {})",
            info.track_type
        ));
    }
    let mut out =
        vec![0f32; info.num_tracks as usize * info.num_samples as usize * TRANSFORM_STRIDE];
    // SAFETY: `out` holds exactly `out.len()` floats and the shim checks that it is large enough.
    let rc = unsafe {
        ffi::fluorite_acl_transforms(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len())
    };
    if rc != 0 {
        return Err(format!("ACL transform decompression failed ({rc})"));
    }
    Ok((info, out))
}

/// Every sample of a scalar buffer: `out[sample * tracks + track]`.
#[cfg(feature = "acl")]
pub fn floats(data: &[u8]) -> Result<(AclInfo, Vec<f32>), String> {
    let info = info(data)?;
    if info.track_type != 0 {
        return Err(format!(
            "not a float1 buffer (track type {})",
            info.track_type
        ));
    }
    let mut out = vec![0f32; info.num_tracks as usize * info.num_samples as usize];
    // SAFETY: as in `transforms`.
    let rc =
        unsafe { ffi::fluorite_acl_floats(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len()) };
    if rc != 0 {
        return Err(format!("ACL float decompression failed ({rc})"));
    }
    Ok((info, out))
}

#[cfg(not(feature = "acl"))]
pub fn transforms(_data: &[u8]) -> Result<(AclInfo, Vec<f32>), String> {
    Err("built without the `acl` feature (cargo build --features acl)".into())
}
