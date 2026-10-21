use crate::extraction::mesh::{
    CompressedSkin, DecodedMesh, PackedBitVector, decode_compressed_skin, decode_mesh,
    decode_scalar, half_to_f32, reconstruct_unit, split_uvs, write_glb,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::f32::consts::FRAC_1_SQRT_2;

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn glb_has_valid_chunks_and_converted_triangle() {
    let mesh = DecodedMesh {
        name: "triangle".into(),
        positions: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        normals: None,
        tangents: None,
        colors: None,
        uvs: vec![],
        submeshes: vec![vec![0, 1, 2]],
    };
    let glb = write_glb(&mesh);
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32_at(&glb, 4), 2);
    assert_eq!(u32_at(&glb, 8) as usize, glb.len());
    let json_length = u32_at(&glb, 12) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_length]).unwrap();
    assert_eq!(json["accessors"].as_array().unwrap().len(), 2);
    // Every accessor must point at an existing buffer view.
    let views = json["bufferViews"].as_array().unwrap().len() as u64;
    for accessor in json["accessors"].as_array().unwrap() {
        assert!(accessor["bufferView"].as_u64().unwrap() < views);
    }
    assert_eq!(json["accessors"][1]["bufferView"], 1);
    assert_eq!(json["meshes"][0]["primitives"][0]["mode"], 4);
    let bin_start = 20 + json_length + 8;
    assert_eq!(
        f32::from_le_bytes(glb[bin_start..bin_start + 4].try_into().unwrap()),
        -1.0
    );
    let index_view = json["bufferViews"][1]["byteOffset"].as_u64().unwrap() as usize;
    assert_eq!(
        &glb[bin_start + index_view..bin_start + index_view + 6],
        &[0, 0, 2, 0, 1, 0]
    );
}

#[test]
fn half_and_normalized_vertex_formats_decode() {
    assert!((half_to_f32(0x3c00) - 1.0).abs() < f32::EPSILON);
    assert!((half_to_f32(0xc000) + 2.0).abs() < f32::EPSILON);
    assert!((decode_scalar(2, &[128]).unwrap() - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(decode_scalar(3, &[0x80]).unwrap(), -1.0);
    assert_eq!(decode_scalar(5, &[0, 0x80]).unwrap(), -1.0);
}

#[test]
fn malformed_mesh_does_not_panic() {
    let object = serde_json::json!({});
    assert!(decode_mesh(&object, &HashMap::new()).is_err());
    assert!(decode_scalar(0, &[0, 0, 0]).is_err());
}

#[test]
#[ignore]
fn fixtures_decode_and_write_glb() {
    use crate::extraction::unity::SerializedFile;
    use std::fs;
    use std::path::Path;
    let root = std::env::var("MESH_FIXTURES").expect("set MESH_FIXTURES");
    let root = Path::new(&root);
    let resources = fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.ends_with(".resS") {
                return None;
            }
            Some((name, fs::read(e.path()).ok()?))
        })
        .collect::<HashMap<_, _>>();
    for file in fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("sf_"))
    {
        let sf = SerializedFile::parse(fs::read(file.path()).unwrap()).unwrap();
        for info in sf.objects_of_type(43) {
            let object = sf.read_object(info).unwrap();
            let mesh = match decode_mesh(&object, &resources) {
                Err(error) if error == "mesh has no vertices" => continue,
                result => result.unwrap(),
            };
            // Decoded positions must reproduce Unity's own bounding box.
            let aabb = &object["m_LocalAABB"];
            for (axis, key) in ["x", "y", "z"].iter().enumerate() {
                let center = aabb["m_Center"][key].as_f64().unwrap() as f32;
                let extent = aabb["m_Extent"][key].as_f64().unwrap() as f32;
                let values = mesh.positions.iter().map(|p| p[axis]);
                let min = values.clone().fold(f32::INFINITY, f32::min);
                let max = values.fold(f32::NEG_INFINITY, f32::max);
                let tolerance = 1e-3 * (center.abs() + extent).max(1.0);
                assert!(
                    (min - (center - extent)).abs() < tolerance,
                    "{} {key} min",
                    mesh.name
                );
                assert!(
                    (max - (center + extent)).abs() < tolerance,
                    "{} {key} max",
                    mesh.name
                );
            }
            assert!(
                mesh.submeshes
                    .iter()
                    .flatten()
                    .all(|&i| i < mesh.positions.len() as u32)
            );
            if let Some(normals) = &mesh.normals {
                assert!(normals.iter().all(|n| {
                    ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 0.02
                }));
            }
            let triangles: usize = mesh.submeshes.iter().map(|s| s.len() / 3).sum();
            println!(
                "{} verts={} tris={} normals={} tangents={} colors={} uvs={}",
                mesh.name,
                mesh.positions.len(),
                triangles,
                mesh.normals.is_some(),
                mesh.tangents.is_some(),
                mesh.colors.is_some(),
                mesh.uvs.len()
            );
            let glb = write_glb(&mesh);
            assert_eq!(&glb[..4], b"glTF");
            assert_eq!(u32_at(&glb, 8) as usize, glb.len());
            let json_size = u32_at(&glb, 12) as usize;
            assert!(serde_json::from_slice::<serde_json::Value>(&glb[20..20 + json_size]).is_ok());
        }
    }
}

/// Pack `values` of `bits` bits each, least-significant bit first, as Unity does.
fn pack(values: &[u32], bits: u32) -> Vec<u8> {
    let mut out = vec![0u8; (values.len() * bits as usize).div_ceil(8)];
    for (i, &value) in values.iter().enumerate() {
        for b in 0..bits as usize {
            if value >> b & 1 != 0 {
                let pos = i * bits as usize + b;
                out[pos / 8] |= 1 << (pos % 8);
            }
        }
    }
    out
}

fn ints(values: &[u32], bits: u32) -> PackedBitVector {
    PackedBitVector {
        num_items: values.len(),
        bit_size: bits,
        data: pack(values, bits),
        range: 0.0,
        start: 0.0,
    }
}

/// A float `PackedBitVector` value quantizing `values` onto `[start, start + range]`.
fn float_vector(values: &[f32], bits: u32, start: f32, range: f32) -> Value {
    let max = ((1u64 << bits) - 1) as f32;
    let quantized: Vec<u32> = values
        .iter()
        .map(|v| ((v - start) / range * max).round() as u32)
        .collect();
    json!({
        "m_NumItems": values.len(),
        "m_BitSize": bits,
        "m_Data": pack(&quantized, bits),
        "m_Range": range,
        "m_Start": start,
    })
}

fn int_vector(values: &[u32], bits: u32) -> Value {
    json!({"m_NumItems": values.len(), "m_BitSize": bits, "m_Data": pack(values, bits)})
}

fn empty_vector() -> Value {
    json!({"m_NumItems": 0, "m_BitSize": 0, "m_Data": [], "m_Range": 0.0, "m_Start": 0.0})
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() <= tolerance, "{actual:?} != {expected:?}");
    }
}

#[test]
fn packed_bit_vector_unpacks_ints_lsb_first() {
    // 5, 2, 7 as 3-bit fields: 0b11_010_101, then the top bit of 7.
    let v = PackedBitVector {
        num_items: 3,
        bit_size: 3,
        data: vec![0b1101_0101, 0b0000_0001],
        range: 0.0,
        start: 0.0,
    };
    assert_eq!(v.unpack_ints().unwrap(), vec![5, 2, 7]);
    // Fields straddling byte boundaries, up to the full 32 bits.
    let tens = [0x3ff, 0, 0x155, 0x2aa, 1];
    assert_eq!(ints(&tens, 10).unpack_ints().unwrap(), tens);
    let wide = [u32::MAX, 0x8000_0001, 7];
    assert_eq!(ints(&wide, 32).unpack_ints().unwrap(), wide);
    assert_eq!(ints(&[0, 0], 0).unpack_ints().unwrap(), vec![0, 0]);
}

#[test]
fn packed_bit_vector_rejects_short_or_oversized_data() {
    let mut v = ints(&[1, 2, 3], 8);
    v.data.pop();
    assert!(v.unpack_ints().is_err());
    let v = PackedBitVector {
        bit_size: 33,
        ..ints(&[1], 8)
    };
    assert!(v.unpack_ints().is_err());
}

#[test]
fn packed_bit_vector_dequantizes_floats() {
    let v = PackedBitVector {
        range: 2.0,
        start: -1.0,
        ..ints(&[0, 255, 51], 8)
    };
    assert_close(
        &v.unpack_floats().unwrap(),
        &[-1.0, 1.0, -1.0 + 102.0 / 255.0],
        1e-6,
    );
    // Zero-bit vectors hold only their start value.
    let v = PackedBitVector {
        start: 0.25,
        ..ints(&[0, 0], 0)
    };
    assert_eq!(v.unpack_floats().unwrap(), vec![0.25, 0.25]);
}

#[test]
fn normal_sign_reconstructs_z() {
    assert_close(&reconstruct_unit(0.6, 0.0, true), &[0.6, 0.0, 0.8], 1e-6);
    assert_close(
        &reconstruct_unit(0.0, -0.6, false),
        &[0.0, -0.6, -0.8],
        1e-6,
    );
    // Quantization can push x² + y² past 1: z is 0 and x, y are renormalized.
    assert_close(
        &reconstruct_unit(0.8, 0.8, false),
        &[FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.0],
        1e-5,
    );
}

#[test]
fn uv_info_selects_channels_and_dimensions() {
    // uv0: present, 2D (0b0101); uv1: absent; uv2: present, 3D (0b0110).
    let info = 0x5 | 0x6 << 8;
    let floats = [0.1, 0.2, 0.3, 0.4, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let uvs = split_uvs(&floats, info, 2);
    assert_eq!(
        uvs,
        vec![vec![[0.1, 0.2], [0.3, 0.4]], vec![[1.0, 2.0], [4.0, 5.0]]]
    );
    // Without m_UVInfo, the floats are uv0 then uv1, both 2D.
    let uvs = split_uvs(&floats[..8], 0, 2);
    assert_eq!(uvs.len(), 2);
    assert_eq!(uvs[1], vec![[1.0, 2.0], [3.0, 4.0]]);
}

fn synthetic_compressed_mesh() -> Value {
    let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    json!({
        "m_Name": "packed",
        "m_MeshCompression": 2,
        "m_IndexFormat": 0,
        "m_SubMeshes": [{"firstByte": 0, "indexCount": 3, "topology": 0, "baseVertex": 0}],
        "m_VertexData": {"m_VertexCount": 0, "m_Channels": [], "m_DataSize": []},
        "m_CompressedMesh": {
            "m_Vertices": float_vector(&positions, 16, -0.5, 2.0),
            "m_UV": float_vector(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.25], 10, 0.0, 1.0),
            "m_UVInfo": 5,
            // (0, 0, ±1), (0.6, 0, -0.8), (0, 0.6, 0.8)
            "m_Normals": float_vector(&[0.0, 0.0, 0.6, 0.0, 0.0, 0.6], 8, -1.0, 2.0),
            "m_NormalSigns": int_vector(&[0, 0, 1], 1),
            "m_Tangents": float_vector(&[1.0, 0.0, 0.0, 1.0, 0.6, 0.0], 8, -1.0, 2.0),
            // Per vertex: z sign, then w sign.
            "m_TangentSigns": int_vector(&[1, 1, 1, 0, 0, 1], 1),
            "m_FloatColors": float_vector(
                &[1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.5], 8, 0.0, 1.0,
            ),
            "m_Triangles": int_vector(&[0, 1, 2], 2),
            // Vertex 0: one bone; vertex 1: two that sum to 31; vertex 2: three short of 31,
            // so a fourth weight and bone index are implied.
            "m_Weights": int_vector(&[31, 15, 16, 10, 10, 5], 5),
            "m_BoneIndices": int_vector(&[3, 1, 2, 0, 1, 2, 4], 3),
            "m_Colors": empty_vector(),
        },
    })
}

#[test]
fn synthetic_compressed_mesh_decodes_end_to_end() {
    let object = synthetic_compressed_mesh();
    let mesh = decode_mesh(&object, &HashMap::new()).unwrap();
    assert_eq!(mesh.name, "packed");
    let step = 2.0 / 65535.0;
    assert_close(
        mesh.positions.as_flattened(),
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        step,
    );
    let normals = mesh.normals.as_ref().unwrap();
    let step = 2.0 / 255.0;
    assert_close(
        normals.as_flattened(),
        &[0.0, 0.0, -1.0, 0.6, 0.0, -0.8, 0.0, 0.6, 0.8],
        2.0 * step,
    );
    let tangents = mesh.tangents.as_ref().unwrap();
    assert_close(
        tangents.as_flattened(),
        &[1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.6, 0.0, -0.8, 1.0],
        2.0 * step,
    );
    assert_eq!(mesh.uvs.len(), 1);
    assert_close(
        mesh.uvs[0].as_flattened(),
        &[0.0, 0.0, 1.0, 0.0, 0.0, 0.25],
        1.0 / 1023.0,
    );
    let colors = mesh.colors.as_ref().unwrap();
    assert_close(colors[2].as_slice(), &[0.0, 0.0, 1.0, 0.5], 1.0 / 255.0);
    assert_eq!(mesh.submeshes, vec![vec![0, 1, 2]]);

    let skin = decode_compressed_skin(&object).unwrap().unwrap();
    assert_eq!(skin.joints, vec![[3, 0, 0, 0], [1, 2, 0, 0], [0, 1, 2, 4]]);
    let w = |n: f32| n / 31.0;
    assert_eq!(
        skin,
        CompressedSkin {
            joints: skin.joints.clone(),
            weights: vec![
                [1.0, 0.0, 0.0, 0.0],
                [w(15.0), w(16.0), 0.0, 0.0],
                [w(10.0), w(10.0), w(5.0), w(6.0)],
            ],
        }
    );

    // The glTF writer applies the usual conventions: X mirrored, winding reversed, V flipped.
    let glb = write_glb(&mesh);
    let json_length = u32_at(&glb, 12) as usize;
    let doc: Value = serde_json::from_slice(&glb[20..20 + json_length]).unwrap();
    let bin = &glb[20 + json_length + 8..];
    let floats = |accessor: &Value, n: usize| -> Vec<f32> {
        let view = &doc["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        bin[start..start + n * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    };
    let attributes = &doc["meshes"][0]["primitives"][0]["attributes"];
    let accessor = |name: &str| &doc["accessors"][attributes[name].as_u64().unwrap() as usize];
    let positions = floats(accessor("POSITION"), 9);
    assert!(positions[3] < -0.99, "x mirrored: {positions:?}");
    let uv = floats(accessor("TEXCOORD_0"), 6);
    assert!((uv[5] - 0.75).abs() < 2e-3, "v flipped: {uv:?}");
    let indices = &doc["accessors"][doc["meshes"][0]["primitives"][0]["indices"]
        .as_u64()
        .unwrap() as usize];
    let view = &doc["bufferViews"][indices["bufferView"].as_u64().unwrap() as usize];
    let start = view["byteOffset"].as_u64().unwrap() as usize;
    assert_eq!(&bin[start..start + 6], &[0, 0, 2, 0, 1, 0]);
}

#[test]
fn compressed_skin_must_cover_every_vertex() {
    let mut object = synthetic_compressed_mesh();
    object["m_CompressedMesh"]["m_Weights"] = int_vector(&[31, 31], 5);
    assert!(decode_compressed_skin(&object).is_err());
    object["m_CompressedMesh"]["m_Weights"] = empty_vector();
    assert_eq!(decode_compressed_skin(&object).unwrap(), None);
}

#[test]
fn empty_compressed_mesh_falls_back_to_vertex_data() {
    // Flagged compressed (Endfield writes 4 and 5), but the geometry is in m_VertexData.
    let data: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let object = json!({
        "m_Name": "plain",
        "m_MeshCompression": 4,
        "m_IndexFormat": 0,
        "m_IndexBuffer": [0, 0, 1, 0, 2, 0],
        "m_SubMeshes": [{"firstByte": 0, "indexCount": 3, "topology": 0, "baseVertex": 0}],
        "m_VertexData": {
            "m_VertexCount": 3,
            "m_Channels": [{"stream": 0, "offset": 0, "format": 0, "dimension": 3}],
            "m_DataSize": data,
        },
        "m_CompressedMesh": {"m_Vertices": empty_vector(), "m_Triangles": empty_vector()},
    });
    let mesh = decode_mesh(&object, &HashMap::new()).unwrap();
    assert_eq!(mesh.positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(mesh.submeshes, vec![vec![0, 1, 2]]);
}

#[test]
fn packed_normal_channel_decodes_octahedral() {
    // (0.6, 0, -0.8) folds to octahedral (1, 4/7); the upper 12 bits (tangent frame) are set
    // and must be ignored.
    let folded = 511 | 292 << 10 | 0xfff << 20;
    // A word from Si's S_actor_jsspsi_cloth_01_lod0, whose averaged face normal is
    // (-0.558, -0.190, -0.808).
    let words: [u32; 3] = [0, folded, 0xeb1a_e640];
    let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let data: Vec<u8> = positions
        .iter()
        .zip(words)
        .flat_map(|(p, w)| {
            p.iter()
                .flat_map(|f| f.to_le_bytes())
                .chain(w.to_le_bytes())
                .collect::<Vec<_>>()
        })
        .collect();
    let object = json!({
        "m_Name": "packed_normals",
        "m_IndexFormat": 0,
        "m_IndexBuffer": [0, 0, 1, 0, 2, 0],
        "m_SubMeshes": [{"firstByte": 0, "indexCount": 3, "topology": 0, "baseVertex": 0}],
        "m_VertexData": {
            "m_VertexCount": 3,
            "m_Channels": [
                {"stream": 0, "offset": 0, "format": 0, "dimension": 3},
                // High nibble 3: one packed 32-bit word.
                {"stream": 0, "offset": 12, "format": 0, "dimension": 0x31},
            ],
            "m_DataSize": data,
        },
        "m_CompressedMesh": {"m_Vertices": empty_vector(), "m_Triangles": empty_vector()},
    });
    let mesh = decode_mesh(&object, &HashMap::new()).unwrap();
    let normals = mesh.normals.unwrap();
    assert_close(&normals[0], &[0.0, 0.0, 1.0], 1e-6);
    assert_close(&normals[1], &[0.6, 0.0, -0.8], 2e-3);
    assert_close(&normals[2], &[-0.558, -0.190, -0.808], 5e-3);
}
