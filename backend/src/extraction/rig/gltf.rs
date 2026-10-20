//! A minimal glTF 2.0 document builder with a single binary buffer, written as GLB.
use serde_json::{Map, Value, json};

pub const FLOAT: u32 = 5126;
pub const UNSIGNED_BYTE: u32 = 5121;
pub const UNSIGNED_SHORT: u32 = 5123;
pub const UNSIGNED_INT: u32 = 5125;

/// `bufferView.target` for vertex attributes.
pub const ARRAY_BUFFER: u32 = 34962;
/// `bufferView.target` for indices.
pub const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// A glTF document under construction. Top-level arrays are plain JSON values.
#[derive(Default)]
pub struct Builder {
    pub bin: Vec<u8>,
    pub buffer_views: Vec<Value>,
    pub accessors: Vec<Value>,
    pub nodes: Vec<Value>,
    pub meshes: Vec<Value>,
    pub skins: Vec<Value>,
    pub materials: Vec<Value>,
    pub textures: Vec<Value>,
    pub images: Vec<Value>,
    pub samplers: Vec<Value>,
    pub animations: Vec<Value>,
    pub extensions_used: Vec<String>,
}

impl Builder {
    fn align(&mut self, alignment: usize) {
        while !self.bin.len().is_multiple_of(alignment) {
            self.bin.push(0);
        }
    }

    /// Append raw bytes as a new buffer view and return its index.
    pub fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align(4);
        let mut view =
            json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(bytes);
        self.buffer_views.push(view);
        self.buffer_views.len() - 1
    }

    fn accessor(&mut self, mut accessor: Value) -> usize {
        if let Some(obj) = accessor.as_object_mut() {
            obj.retain(|_, v| !v.is_null());
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Float vectors (`VEC2`/`VEC3`/`VEC4`/`MAT4`) as an accessor; `with_bounds` adds min/max.
    pub fn floats<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        kind: &str,
        target: Option<u32>,
        with_bounds: bool,
    ) -> usize {
        let mut bytes = Vec::with_capacity(values.len() * N * 4);
        for v in values {
            for x in v {
                bytes.extend_from_slice(&x.to_le_bytes());
            }
        }
        let view = self.view(&bytes, target);
        let (min, max) = if with_bounds && !values.is_empty() {
            let mut min = [f32::INFINITY; N];
            let mut max = [f32::NEG_INFINITY; N];
            for v in values {
                for i in 0..N {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            (json!(min.to_vec()), json!(max.to_vec()))
        } else {
            (Value::Null, Value::Null)
        };
        self.accessor(json!({
            "bufferView": view, "componentType": FLOAT, "count": values.len(),
            "type": kind, "min": min, "max": max
        }))
    }

    /// Joint indices as `VEC4` of unsigned bytes (≤ 255 joints) or shorts.
    pub fn joints(&mut self, values: &[[u16; 4]], joint_count: usize) -> usize {
        let byte = joint_count <= 256;
        let mut bytes = Vec::with_capacity(values.len() * if byte { 4 } else { 8 });
        for v in values {
            for &j in v {
                if byte {
                    bytes.push(j as u8);
                } else {
                    bytes.extend_from_slice(&j.to_le_bytes());
                }
            }
        }
        let view = self.view(&bytes, Some(ARRAY_BUFFER));
        let component = if byte { UNSIGNED_BYTE } else { UNSIGNED_SHORT };
        self.accessor(json!({
            "bufferView": view, "componentType": component, "count": values.len(), "type": "VEC4"
        }))
    }

    /// Triangle indices, narrowed to u16 when possible.
    pub fn indices(&mut self, values: &[u32]) -> usize {
        let short = values.iter().all(|&i| i <= u32::from(u16::MAX));
        let mut bytes = Vec::with_capacity(values.len() * if short { 2 } else { 4 });
        for &i in values {
            if short {
                bytes.extend_from_slice(&(i as u16).to_le_bytes());
            } else {
                bytes.extend_from_slice(&i.to_le_bytes());
            }
        }
        let view = self.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
        let component = if short { UNSIGNED_SHORT } else { UNSIGNED_INT };
        self.accessor(json!({
            "bufferView": view, "componentType": component, "count": values.len(), "type": "SCALAR"
        }))
    }

    /// Embed an encoded image (PNG/WebP) and return its texture index.
    pub fn texture(&mut self, bytes: &[u8], mime: &str, name: &str) -> usize {
        if self.samplers.is_empty() {
            // LINEAR_MIPMAP_LINEAR / LINEAR, REPEAT.
            self.samplers.push(
                json!({"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}),
            );
        }
        let view = self.view(bytes, None);
        self.images
            .push(json!({"name": name, "mimeType": mime, "bufferView": view}));
        let image = self.images.len() - 1;
        let texture = if mime == "image/webp" {
            if !self.extensions_used.iter().any(|e| e == "EXT_texture_webp") {
                self.extensions_used.push("EXT_texture_webp".into());
            }
            json!({"name": name, "sampler": 0, "extensions": {"EXT_texture_webp": {"source": image}}})
        } else {
            json!({"name": name, "sampler": 0, "source": image})
        };
        self.textures.push(texture);
        self.textures.len() - 1
    }

    pub fn node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Assemble the JSON document and the GLB container.
    #[must_use]
    pub fn finish(mut self, scene_nodes: Vec<usize>, asset_extras: Value) -> (Value, Vec<u8>) {
        self.align(4);
        let mut doc = Map::new();
        doc.insert(
            "asset".into(),
            json!({"version": "2.0", "generator": "fluorite rig exporter", "extras": asset_extras}),
        );
        doc.insert("scene".into(), json!(0));
        doc.insert("scenes".into(), json!([{"nodes": scene_nodes}]));
        let arrays = [
            ("nodes", self.nodes),
            ("meshes", self.meshes),
            ("skins", self.skins),
            ("materials", self.materials),
            ("textures", self.textures),
            ("images", self.images),
            ("samplers", self.samplers),
            ("animations", self.animations),
            ("accessors", self.accessors),
            ("bufferViews", self.buffer_views),
        ];
        for (key, values) in arrays {
            if !values.is_empty() {
                doc.insert(key.into(), Value::Array(values));
            }
        }
        if !self.bin.is_empty() {
            doc.insert("buffers".into(), json!([{"byteLength": self.bin.len()}]));
        }
        if !self.extensions_used.is_empty() {
            doc.insert("extensionsUsed".into(), json!(self.extensions_used));
            doc.insert("extensionsRequired".into(), json!(self.extensions_used));
        }
        let doc = Value::Object(doc);
        let glb = glb_bytes(&doc, &self.bin);
        (doc, glb)
    }
}

/// Serialize a document and binary chunk as GLB.
#[must_use]
pub fn glb_bytes(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(doc).unwrap_or_default();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let bin_len = if bin.is_empty() { 0 } else { 8 + bin.len() };
    let total = 12 + 8 + json_bytes.len() + bin_len;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
    }
    out
}

/// Parse a GLB back into its JSON document and binary chunk.
pub fn parse_glb(data: &[u8]) -> Result<(Value, &[u8]), String> {
    let u32_at = |off: usize| -> Result<usize, String> {
        data.get(off..off + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
            .ok_or_else(|| "truncated GLB".to_string())
    };
    if data.get(0..4) != Some(b"glTF") {
        return Err("not a GLB".into());
    }
    let json_len = u32_at(12)?;
    let json = data.get(20..20 + json_len).ok_or("truncated JSON chunk")?;
    let doc: Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let bin_start = 20 + json_len;
    let bin = if data.len() >= bin_start + 8 {
        let len = u32_at(bin_start)?;
        data.get(bin_start + 8..bin_start + 8 + len)
            .ok_or("truncated BIN chunk")?
    } else {
        &[]
    };
    Ok((doc, bin))
}

/// Structural checks of a document: index references in range, accessors inside their views,
/// skins with matching IBM counts, primitives with consistent attribute counts.
pub fn check_document(doc: &Value, bin_len: usize) -> Vec<String> {
    let mut errors = Vec::new();
    let len = |key: &str| doc.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let arr = |key: &str| -> Vec<Value> {
        doc.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let idx = |v: &Value| v.as_u64().map(|n| n as usize);
    let views = arr("bufferViews");
    for (i, view) in views.iter().enumerate() {
        let off = view.get("byteOffset").and_then(idx).unwrap_or(0);
        let length = view.get("byteLength").and_then(idx).unwrap_or(0);
        if off + length > bin_len {
            errors.push(format!("bufferView {i} exceeds buffer"));
        }
    }
    let accessors = arr("accessors");
    for (i, a) in accessors.iter().enumerate() {
        let Some(view) = a.get("bufferView").and_then(idx) else {
            continue;
        };
        let Some(v) = views.get(view) else {
            errors.push(format!("accessor {i} references missing bufferView {view}"));
            continue;
        };
        let comps = match a.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") | Some("MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            _ => {
                errors.push(format!("accessor {i} has invalid type"));
                continue;
            }
        };
        let size = match a.get("componentType").and_then(Value::as_u64) {
            Some(5120 | 5121) => 1,
            Some(5122 | 5123) => 2,
            Some(5125 | 5126) => 4,
            _ => {
                errors.push(format!("accessor {i} has invalid componentType"));
                continue;
            }
        };
        let count = a.get("count").and_then(idx).unwrap_or(0);
        let need = a.get("byteOffset").and_then(idx).unwrap_or(0) + count * comps * size;
        if need > v.get("byteLength").and_then(idx).unwrap_or(0) {
            errors.push(format!("accessor {i} exceeds bufferView {view}"));
        }
    }
    let nodes = arr("nodes");
    let mut parent_count = vec![0usize; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for c in node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match idx(c) {
                Some(c) if c < nodes.len() => parent_count[c] += 1,
                _ => errors.push(format!("node {i} has invalid child {c}")),
            }
        }
        for (key, limit) in [("mesh", len("meshes")), ("skin", len("skins"))] {
            if let Some(n) = node.get(key).and_then(idx)
                && n >= limit
            {
                errors.push(format!("node {i} references missing {key} {n}"));
            }
        }
    }
    for (i, n) in parent_count.iter().enumerate() {
        if *n > 1 {
            errors.push(format!("node {i} has {n} parents"));
        }
    }
    for (i, skin) in arr("skins").iter().enumerate() {
        let joints = skin
            .get("joints")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if joints == 0 {
            errors.push(format!("skin {i} has no joints"));
        }
        for j in skin
            .get("joints")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if idx(j).is_none_or(|j| j >= nodes.len()) {
                errors.push(format!("skin {i} has invalid joint {j}"));
            }
        }
        if let Some(ibm) = skin.get("inverseBindMatrices").and_then(idx) {
            let count = accessors
                .get(ibm)
                .and_then(|a| a.get("count"))
                .and_then(idx);
            if count != Some(joints) {
                errors.push(format!("skin {i}: IBM count {count:?} != joints {joints}"));
            }
        }
    }
    let materials = len("materials");
    for (m, mesh) in arr("meshes").iter().enumerate() {
        for (p, prim) in mesh
            .get("primitives")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let mut counts = Vec::new();
            for (name, a) in prim
                .get("attributes")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                match idx(a).and_then(|a| accessors.get(a)) {
                    Some(acc) => counts.push(acc.get("count").and_then(idx).unwrap_or(0)),
                    None => errors.push(format!("mesh {m} prim {p}: {name} missing accessor")),
                }
            }
            if counts.windows(2).any(|w| w[0] != w[1]) {
                errors.push(format!("mesh {m} prim {p}: attribute counts differ"));
            }
            if let Some(mat) = prim.get("material").and_then(idx)
                && mat >= materials
            {
                errors.push(format!("mesh {m} prim {p}: missing material {mat}"));
            }
            if let Some(ind) = prim.get("indices").and_then(idx)
                && ind >= accessors.len()
            {
                errors.push(format!("mesh {m} prim {p}: missing indices accessor"));
            }
        }
    }
    errors
}
