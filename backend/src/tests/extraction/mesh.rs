use crate::extraction::mesh::{DecodedMesh, decode_mesh, decode_scalar, half_to_f32, write_glb};
use std::collections::HashMap;

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
    let object = unity_asset_binary::object::UnityObject::from_raw(43, 1, vec![0]);
    assert!(decode_mesh(&object, &HashMap::new()).is_err());
    assert!(decode_scalar(0, &[0, 0, 0]).is_err());
}

#[test]
#[ignore]
fn fixtures_decode_and_write_glb() {
    use std::fs;
    use std::path::Path;
    use unity_asset_binary::asset::SerializedFileParser;
    use unity_asset_binary::object::ObjectHandle;
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
        let sf = SerializedFileParser::from_bytes(fs::read(file.path()).unwrap()).unwrap();
        for info in sf.objects_of_type(43) {
            let object = ObjectHandle::new(&sf, info).read().unwrap();
            let mesh = match decode_mesh(&object, &resources) {
                Err(error) if error == "mesh has no vertices" => continue,
                result => result.unwrap(),
            };
            // Decoded positions must reproduce Unity's own bounding box.
            let aabb = serde_json::to_value(object.get("m_LocalAABB").unwrap()).unwrap();
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
