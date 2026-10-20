//! Skeletons of model imports that ship only an Avatar (no GameObjects or Transforms).
use super::export::ObjRef;
use serde_json::Value;
use std::collections::HashMap;

/// Avatar skeleton: per node its path hash, parent and local pose, plus the TOS
/// (path hash -> `Root/Bip001/...` path).
pub struct Skeleton {
    pub ids: Vec<u32>,
    pub parents: Vec<i64>,
    /// Local translation, rotation (x, y, z, w) and scale, in Unity space.
    pub local: Vec<([f32; 3], [f32; 4], [f32; 3])>,
    pub tos: HashMap<u32, String>,
}

/// A synthetic object ref standing for the skeleton node with this path hash, so bones of
/// Avatar-only models go through the same joint lookup as Transforms.
#[must_use]
pub fn bone_ref(hash: u32) -> ObjRef {
    ObjRef {
        file: usize::MAX,
        path_id: i64::from(hash),
    }
}

fn vec3(v: Option<&Value>, default: f32) -> [f32; 3] {
    let get = |k: &str| {
        v.and_then(|v| v.get(k))
            .and_then(Value::as_f64)
            .map_or(default, |x| x as f32)
    };
    [get("x"), get("y"), get("z")]
}

impl Skeleton {
    /// Read `m_Avatar.m_AvatarSkeleton` (ids, parents), `m_AvatarSkeletonPose` (local poses,
    /// falling back to `m_DefaultPose`) and `m_TOS`.
    pub fn from_avatar(avatar: &Value) -> Result<Self, String> {
        let skeleton = avatar
            .pointer("/m_Avatar/m_AvatarSkeleton/data")
            .ok_or("avatar has no skeleton")?;
        let ids: Vec<u32> = skeleton
            .get("m_ID")
            .and_then(Value::as_array)
            .ok_or("skeleton has no ids")?
            .iter()
            .map(|v| v.as_u64().unwrap_or(0) as u32)
            .collect();
        let parents: Vec<i64> = skeleton
            .get("m_Node")
            .and_then(Value::as_array)
            .ok_or("skeleton has no nodes")?
            .iter()
            .map(|n| n.get("m_ParentId").and_then(Value::as_i64).unwrap_or(-1))
            .collect();
        if ids.is_empty() || ids.len() != parents.len() {
            return Err(format!(
                "skeleton has {} ids for {} nodes",
                ids.len(),
                parents.len()
            ));
        }
        // Parents must precede children for a well-formed tree.
        if parents
            .iter()
            .enumerate()
            .any(|(i, &p)| p >= i as i64 || (i > 0 && p < 0))
        {
            return Err("skeleton parents are not topologically ordered".into());
        }
        let pose = [
            "/m_Avatar/m_AvatarSkeletonPose/data/m_X",
            "/m_Avatar/m_DefaultPose/data/m_X",
        ]
        .iter()
        .find_map(|p| {
            avatar
                .pointer(p)
                .and_then(Value::as_array)
                .filter(|a| a.len() == ids.len())
        })
        .ok_or("avatar has no skeleton pose")?;
        let local = pose
            .iter()
            .map(|x| {
                let q = x.get("q");
                let get = |k: &str, d: f32| {
                    q.and_then(|q| q.get(k))
                        .and_then(Value::as_f64)
                        .map_or(d, |v| v as f32)
                };
                (
                    vec3(x.get("t"), 0.0),
                    [get("x", 0.0), get("y", 0.0), get("z", 0.0), get("w", 1.0)],
                    vec3(x.get("s"), 1.0),
                )
            })
            .collect();
        let mut tos = HashMap::new();
        match avatar.get("m_TOS") {
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    if let (Ok(h), Some(p)) = (k.parse::<u32>(), v.as_str()) {
                        tos.insert(h, p.to_string());
                    }
                }
            }
            Some(Value::Array(items)) => {
                for pair in items {
                    if let (Some(h), Some(p)) = (
                        pair.get(0).and_then(Value::as_u64),
                        pair.get(1).and_then(Value::as_str),
                    ) {
                        tos.insert(h as u32, p.to_string());
                    }
                }
            }
            _ => {}
        }
        Ok(Self {
            ids,
            parents,
            local,
            tos,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Parent index of node `i` (`None` for the root).
    #[must_use]
    pub fn parent(&self, i: usize) -> Option<usize> {
        usize::try_from(*self.parents.get(i)?).ok()
    }

    /// Node name: the last segment of its TOS path, else a placeholder.
    #[must_use]
    pub fn name(&self, i: usize) -> String {
        self.tos
            .get(&self.ids[i])
            .and_then(|p| p.rsplit('/').next())
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("node_{i}"), str::to_string)
    }
}
