//! Unity (left-handed, Y up) → glTF (right-handed, Y up) conversion.
//!
//! `mesh.rs` converts vertices with the reflection `S = diag(-1, 1, 1)` (negate X). Every
//! transform is conjugated by the same reflection, `M' = S·M·S`, so hierarchy, bind poses and
//! vertices stay consistent:
//! - translation `(x, y, z)` → `(-x, y, z)`
//! - rotation quaternion `(x, y, z, w)` → `(x, -y, -z, w)`
//! - scale unchanged
//! - 4×4 matrix: elements where exactly one of row/column is X change sign.

/// Unity position/translation to glTF.
#[must_use]
pub fn position(v: [f32; 3]) -> [f32; 3] {
    [-v[0], v[1], v[2]]
}

/// Unity quaternion `(x, y, z, w)` to a normalized glTF quaternion.
///
/// A degenerate (zero) quaternion becomes the identity.
#[must_use]
pub fn rotation(q: [f32; 4]) -> [f32; 4] {
    let c = [q[0], -q[1], -q[2], q[3]];
    let len = c.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !len.is_finite() || len < 1e-8 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    c.map(|x| x / len)
}

/// A Unity `Matrix4x4` as serialized (`e<row><col>` fields), row-major `[row][col]`.
pub type RowMajor = [[f32; 4]; 4];

/// Conjugate a row-major Unity matrix by the X reflection.
#[must_use]
pub fn matrix(m: RowMajor) -> RowMajor {
    let s = [-1.0f32, 1.0, 1.0, 1.0];
    let mut out = m;
    for (r, row) in out.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value *= s[r] * s[c];
        }
    }
    out
}

/// Column-major flattening, as glTF stores matrices, with the bottom row forced to `0 0 0 1`
/// when it is already within rounding of it (the validator rejects non-affine IBMs).
#[must_use]
pub fn column_major(m: RowMajor) -> [f32; 16] {
    let mut m = m;
    let bottom = [0.0, 0.0, 0.0, 1.0];
    if m[3].iter().zip(bottom).all(|(a, b)| (a - b).abs() < 1e-4) {
        m[3] = bottom;
    }
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = m[r][c];
        }
    }
    out
}

/// Row-major product `a·b`.
#[must_use]
pub fn mul(a: RowMajor, b: RowMajor) -> RowMajor {
    let mut out = [[0.0; 4]; 4];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    out
}

/// Row-major TRS matrix (column-vector convention, `M = T·R·S`).
#[must_use]
pub fn trs(t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> RowMajor {
    let [x, y, z, w] = q;
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    [
        [r[0][0] * s[0], r[0][1] * s[1], r[0][2] * s[2], t[0]],
        [r[1][0] * s[0], r[1][1] * s[1], r[1][2] * s[2], t[1]],
        [r[2][0] * s[0], r[2][1] * s[1], r[2][2] * s[2], t[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Normalize up to four skin weights in place so they sum to exactly 1 (within f32 rounding).
///
/// Negative or non-finite weights are zeroed and zero-weight slots get joint 0, so indices of
/// non-zero weights stay unique as glTF requires. An all-zero vertex is bound fully to
/// `fallback_joint`. Duplicate joints are merged into the first slot.
pub fn normalize_influences(joints: &mut [u16; 4], weights: &mut [f32; 4], fallback_joint: u16) {
    for w in weights.iter_mut() {
        if !w.is_finite() || *w < 0.0 {
            *w = 0.0;
        }
    }
    for i in 0..4 {
        if weights[i] == 0.0 {
            continue;
        }
        for j in (i + 1)..4 {
            if weights[j] > 0.0 && joints[j] == joints[i] {
                weights[i] += weights[j];
                weights[j] = 0.0;
            }
        }
    }
    let sum: f32 = weights.iter().sum();
    if sum <= 0.0 {
        *joints = [fallback_joint, 0, 0, 0];
        *weights = [1.0, 0.0, 0.0, 0.0];
        return;
    }
    for w in weights.iter_mut() {
        *w /= sum;
    }
    // Put the rounding error on the largest weight so the sum is as close to 1 as f32 allows.
    let largest = (0..4)
        .max_by(|&a, &b| weights[a].total_cmp(&weights[b]))
        .unwrap_or(0);
    let others: f32 = (0..4).filter(|&i| i != largest).map(|i| weights[i]).sum();
    weights[largest] = 1.0 - others;
    for i in 0..4 {
        if weights[i] == 0.0 {
            joints[i] = 0;
        }
    }
}
