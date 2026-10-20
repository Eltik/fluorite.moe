use crate::extraction::rig::convert::{self, RowMajor};
use crate::extraction::rig::entity::{candidates, file_stem, id_of, kind_of};
use crate::extraction::rig::export::split_lod;
use crate::extraction::rig::gltf::{self, ARRAY_BUFFER, Builder};
use crate::extraction::rig::material::normal_map;
use crate::extraction::rig::skin;
use crate::extraction::rig::store::CabRecord;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn apply(m: RowMajor, v: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3];
    }
    out
}

fn unit_quat(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    q.map(|x| x / len)
}

const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

const SAMPLES: &[([f32; 3], [f32; 4], [f32; 3])] = &[
    (
        [0.1, 0.48, -0.05],
        [0.2998, -0.6404, -0.2999, 0.6404],
        [1.0, 1.0, 1.0],
    ),
    (
        [-1.5, 0.0, 2.0],
        [0.0, H, 0.0, H],
        [1.0, 2.0, 0.5],
    ),
    (
        [0.0, 3.0, 0.0],
        [-H, 0.0, 0.0, H],
        [1.0, 1.0, 1.0],
    ),
    ([0.3, -0.2, 0.9], [0.1, 0.2, 0.3, 0.9], [0.8, 0.8, 0.8]),
];

#[test]
fn quaternion_conversion_negates_y_and_z() {
    let q = convert::rotation([0.1, 0.2, 0.3, 0.9]);
    let n = unit_quat([0.1, -0.2, -0.3, 0.9]);
    for i in 0..4 {
        assert!(close(q[i], n[i]));
    }
    assert_eq!(convert::rotation([0.0; 4]), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(convert::position([1.0, 2.0, 3.0]), [-1.0, 2.0, 3.0]);
}

/// Converting a TRS component-wise must equal conjugating its matrix by the X mirror, so
/// node transforms, bind poses and mesh vertices all land in the same right-handed space.
#[test]
fn trs_conversion_matches_matrix_conjugation() {
    for &(t, q, s) in SAMPLES {
        let q = unit_quat(q);
        let unity = convert::trs(t, q, s);
        let via_matrix = convert::matrix(unity);
        let via_trs = convert::trs(convert::position(t), convert::rotation(q), s);
        for r in 0..4 {
            for c in 0..4 {
                assert!(
                    close(via_matrix[r][c], via_trs[r][c]),
                    "[{r}][{c}] {} vs {}",
                    via_matrix[r][c],
                    via_trs[r][c]
                );
            }
        }
    }
}

/// A converted matrix applied to a converted vertex gives the converted transformed vertex.
#[test]
fn matrix_conversion_commutes_with_vertex_conversion() {
    for &(t, q, s) in SAMPLES {
        let m = convert::trs(t, unit_quat(q), s);
        let v = [0.25, -0.5, 1.5];
        let expected = convert::position(apply(m, v));
        let got = apply(convert::matrix(m), convert::position(v));
        for i in 0..3 {
            assert!(close(expected[i], got[i]));
        }
    }
}

/// Skinning a vertex with converted bone world matrices and bind poses gives the converted
/// result of skinning in Unity space.
#[test]
fn skinning_is_preserved_by_conversion() {
    let (t, q, s) = SAMPLES[0];
    let bone_world = convert::mul(
        convert::trs(t, unit_quat(q), s),
        convert::trs([0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
    );
    let bind = convert::trs([0.0, -1.2, 0.1], unit_quat([0.0, 0.0, 0.2, 0.98]), [1.0; 3]);
    let v = [0.1, 1.3, 0.0];
    let unity = apply(convert::mul(bone_world, bind), v);
    let gltf = apply(
        convert::mul(convert::matrix(bone_world), convert::matrix(bind)),
        convert::position(v),
    );
    let expected = convert::position(unity);
    for i in 0..3 {
        assert!(close(expected[i], gltf[i]));
    }
}

#[test]
fn column_major_layout_and_affine_bottom_row() {
    let mut m = convert::trs([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
    m[3] = [1e-7, -1e-7, 0.0, 1.0 + 1e-7];
    let c = convert::column_major(m);
    assert_eq!(&c[12..16], &[1.0, 2.0, 3.0, 1.0]);
    assert_eq!([c[3], c[7], c[11]], [0.0, 0.0, 0.0]);
}

#[test]
fn influence_normalization() {
    let mut j = [3, 5, 7, 9];
    let mut w = [0.5, 0.25, 0.25, 0.25];
    convert::normalize_influences(&mut j, &mut w, 0);
    assert!(close(w.iter().sum(), 1.0));
    assert!(close(w[0], 0.4));

    // Duplicate joints merge; zero-weight slots point at joint 0.
    let mut j = [4, 4, 2, 6];
    let mut w = [0.3, 0.3, 0.4, 0.0];
    convert::normalize_influences(&mut j, &mut w, 0);
    assert_eq!(j, [4, 0, 2, 0]);
    assert!(close(w[0], 0.6) && w[1] == 0.0 && close(w[2], 0.4));

    // Negative and non-finite weights are dropped; empty vertices bind to the fallback.
    let mut j = [1, 2, 3, 4];
    let mut w = [-1.0, f32::NAN, 0.0, 0.0];
    convert::normalize_influences(&mut j, &mut w, 7);
    assert_eq!((j, w), ([7, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
}

/// Channel table `(stream, offset, format, dimension)` with only weights (12) and indices (13).
fn skin_channels(
    weight_format: usize,
    weight_dim: usize,
    index_format: usize,
    index_dim: usize,
) -> Vec<(usize, usize, usize, usize)> {
    let weight_size = if weight_format == 0 { 4 } else { 1 };
    let mut chans = vec![(0, 0, 0, 0); 14];
    chans[12] = (0, 0, weight_format, weight_dim);
    chans[13] = (0, weight_size * weight_dim, index_format, index_dim);
    chans
}

#[test]
fn skin_channels_decode_and_normalize() {
    // Two vertices: float32 x4 weights + uint8 x4 indices (stride 20).
    let mut data = Vec::new();
    for (w, j) in [
        ([0.5f32, 0.5, 0.0, 0.0], [1u8, 2, 0, 0]),
        ([0.2, 0.2, 0.2, 0.2], [0, 1, 2, 3]),
    ] {
        for x in w {
            data.extend_from_slice(&x.to_le_bytes());
        }
        data.extend_from_slice(&j);
    }
    let decoded = skin::decode_for_test(&skin_channels(0, 4, 6, 4), &data, 2).unwrap();
    assert_eq!(decoded.joints, vec![[1, 2, 0, 0], [0, 1, 2, 3]]);
    assert!(close(decoded.weights[1].iter().sum(), 1.0));
    assert!(close(decoded.weights[1][0], 0.25));
    assert_eq!(decoded.renormalized, 1);

    // UNorm8 weights with the last influence implied by the others.
    let data = [128u8, 64, 32, 0, 5, 6, 7, 8];
    let decoded = skin::decode_for_test(
        &skin_channels(2, 3, 6, 4),
        &[data[0..3].to_vec(), vec![5, 6, 7, 8]].concat(),
        1,
    );
    let decoded = decoded.unwrap();
    assert_eq!(decoded.joints[0], [5, 6, 7, 8]);
    assert!(close(decoded.weights[0].iter().sum(), 1.0));
    assert!(decoded.weights[0][3] > 0.0);
    let _ = data;
}

#[test]
fn skin_decode_rejects_short_buffers() {
    assert!(skin::decode_for_test(&skin_channels(0, 4, 6, 4), &[0u8; 10], 1).is_err());
}

#[test]
fn bind_pose_matrix_parsing() {
    let value = json!({"e00": 1.0, "e01": 2.0, "e03": 5.0, "e11": 1.0, "e22": 1.0, "e33": 1.0});
    let m = skin::matrix_value(&value);
    assert_eq!(m[0], [1.0, 2.0, 0.0, 5.0]);
    assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0]);
    let c = convert::matrix(m);
    assert_eq!(c[0], [1.0, -2.0, 0.0, -5.0]);
}

/// A tiny skinned document: two joints, one triangle.
fn skinned_document(ibm_count: usize) -> (Value, Vec<u8>) {
    let mut b = Builder::default();
    let child = b.node(json!({"name": "bone1", "translation": [0.0, 1.0, 0.0]}));
    let root = b.node(json!({"name": "root", "children": [child]}));
    let positions = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let pos = b.floats(&positions, "VEC3", Some(ARRAY_BUFFER), true);
    let joints = b.joints(&[[0, 1, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]], 2);
    let weights = b.floats(
        &[
            [0.5f32, 0.5, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ],
        "VEC4",
        Some(ARRAY_BUFFER),
        false,
    );
    let indices = b.indices(&[0, 1, 2]);
    b.materials.push(json!({"name": "m"}));
    b.meshes.push(json!({"primitives": [{"attributes": {"POSITION": pos, "JOINTS_0": joints, "WEIGHTS_0": weights}, "indices": indices, "material": 0}]}));
    let identity = convert::column_major(convert::trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]));
    let ibm = b.floats(&vec![identity; ibm_count], "MAT4", None, false);
    b.skins
        .push(json!({"joints": [root, child], "inverseBindMatrices": ibm, "skeleton": root}));
    let mesh_node = b.node(json!({"name": "skinned", "mesh": 0, "skin": 0}));
    let (doc, glb) = b.finish(vec![root, mesh_node], json!({"test": true}));
    (doc, glb)
}

#[test]
fn gltf_document_is_structurally_valid() {
    let (doc, glb) = skinned_document(2);
    let (parsed, bin) = gltf::parse_glb(&glb).unwrap();
    assert_eq!(parsed, doc);
    assert_eq!(glb.len() % 4, 0);
    assert_eq!(&glb[0..4], b"glTF");
    assert_eq!(
        u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
        glb.len()
    );
    assert_eq!(
        doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize,
        bin.len()
    );
    assert!(
        gltf::check_document(&doc, bin.len()).is_empty(),
        "{:?}",
        gltf::check_document(&doc, bin.len())
    );
    // Joint indices are bytes for small skeletons, and every view is 4-byte aligned.
    let joints = &doc["accessors"][doc["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"]
        .as_u64()
        .unwrap() as usize];
    assert_eq!(joints["componentType"], 5121);
    for view in doc["bufferViews"].as_array().unwrap() {
        assert_eq!(view["byteOffset"].as_u64().unwrap() % 4, 0);
    }
    assert_eq!(doc["skins"][0]["joints"].as_array().unwrap().len(), 2);
}

#[test]
fn gltf_check_reports_broken_references() {
    let (doc, glb) = skinned_document(3);
    let (_, bin) = gltf::parse_glb(&glb).unwrap();
    let errors = gltf::check_document(&doc, bin.len());
    assert!(errors.iter().any(|e| e.contains("IBM count")), "{errors:?}");

    let (mut doc, glb) = skinned_document(2);
    let (_, bin) = gltf::parse_glb(&glb).unwrap();
    doc["meshes"][0]["primitives"][0]["material"] = json!(9);
    doc["nodes"][1]["children"] = json!([0, 0]);
    let errors = gltf::check_document(&doc, bin.len());
    assert!(
        errors.iter().any(|e| e.contains("missing material")),
        "{errors:?}"
    );
    assert!(errors.iter().any(|e| e.contains("2 parents")), "{errors:?}");
    assert!(!gltf::check_document(&doc, 8).is_empty());
}

#[test]
fn lod_names_and_entity_ids() {
    assert_eq!(
        split_lod("S_actor_chen_body_01_lod0"),
        ("s_actor_chen_body_01".into(), Some(0))
    );
    assert_eq!(split_lod("Body_LOD2"), ("body".into(), Some(2)));
    assert_eq!(split_lod("lodge"), ("lodge".into(), None));
    let c = "assets/beyond/dynamicassets/gameplay/actors/postmodels/enemies/eny_0073_slimerg_postmodel.prefab";
    assert_eq!(
        (kind_of(c), id_of(c).as_str()),
        ("enemy", "eny_0073_slimerg")
    );
    let w = "assets/beyond/dynamicassets/gameplay/prefabs/weapons/wpn_sword_0003.prefab";
    assert_eq!(
        (kind_of(w), id_of(w).as_str()),
        ("weapon", "wpn_sword_0003")
    );
    assert_eq!(
        kind_of("assets/x/postmodels/characters/chr_0005_chen_postmodel.prefab"),
        "operator"
    );
    assert_eq!(file_stem("a/b c"), "a_b_c");
}

#[test]
fn candidates_prefer_local_prefabs_with_objects() {
    let rec =
        |cab: &str, classes: &[(i32, usize)], containers: Vec<(String, i64, i64)>| CabRecord {
            cab: cab.into(),
            chunk: "C".into(),
            sf_size: 0,
            resources: vec![],
            classes: classes.iter().copied().collect::<BTreeMap<_, _>>(),
            containers,
            externals: vec![],
        };
    let path = "assets/x/postmodels/enemies/eny_0001_a_postmodel.prefab".to_string();
    let records = vec![
        rec(
            "CAB-a",
            &[(1, 3)],
            vec![(path.clone(), 0, 42), ("assets/x/t.tga".into(), 0, 1)],
        ),
        rec("CAB-b", &[(28, 1)], vec![(path.clone(), 0, 43)]),
        rec("CAB-c", &[(1, 1)], vec![(path.clone(), 1, 44)]),
    ];
    let found = candidates(&records, "enemies");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sources, vec![("CAB-a".to_string(), 42)]);
    assert_eq!(found[0].id, "eny_0001_a");
}

#[test]
fn bc5_normal_maps_get_reconstructed_z() {
    let mut img = image::RgbaImage::new(4, 4);
    for p in img.pixels_mut() {
        p.0 = [128, 128, 0, 255];
    }
    let n = normal_map(&img, "BC5").unwrap();
    assert!(n.pixels().all(|p| p.0[2] == 255));
    // A colorful albedo is not a normal map.
    for p in img.pixels_mut() {
        p.0 = [250, 20, 30, 255];
    }
    assert!(normal_map(&img, "BC7").is_none());
}

/// Exports every prefab of a store directory and checks structure and skin weights.
///
/// `RIG_FIXTURE_STORE=<store dir> RIG_FIXTURE_FILTER=<substring> cargo test -- --ignored rig_fixture`
#[test]
#[ignore = "needs a scanned store (endfield-models scan) in RIG_FIXTURE_STORE"]
fn rig_fixture_exports_valid_skinned_glbs() {
    use crate::extraction::rig::store::{CabSource, read_index};
    use crate::extraction::rig::{ChunkFiles, ExportOptions, Exporter, ObjRef};
    let Ok(store) = std::env::var("RIG_FIXTURE_STORE") else {
        eprintln!("RIG_FIXTURE_STORE not set");
        return;
    };
    let filter = std::env::var("RIG_FIXTURE_FILTER").unwrap_or_else(|_| "postmodels/".into());
    let store = std::path::PathBuf::from(store);
    let records = read_index(&store).unwrap();
    let found = candidates(&records, &filter);
    assert!(!found.is_empty(), "no prefabs match {filter}");
    let mut source = CabSource::new(Some(store), ChunkFiles::default());
    let mut skinned = 0;
    for cand in found.iter().take(10) {
        let (cab, path_id) = &cand.sources[0];
        let file = source.get(cab).unwrap();
        let Ok(export) = Exporter::new(&mut source, ExportOptions::default()).export(
            ObjRef {
                file,
                path_id: *path_id,
            },
            json!({}),
        ) else {
            continue;
        };
        let (doc, bin) = gltf::parse_glb(&export.glb).unwrap();
        assert!(gltf::check_document(&doc, bin.len()).is_empty());
        for mesh in doc["meshes"].as_array().unwrap() {
            for prim in mesh["primitives"].as_array().unwrap() {
                let Some(w) = prim["attributes"].get("WEIGHTS_0").and_then(Value::as_u64) else {
                    continue;
                };
                skinned += 1;
                let acc = &doc["accessors"][w as usize];
                let view = &doc["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
                let off = view["byteOffset"].as_u64().unwrap() as usize;
                for v in 0..acc["count"].as_u64().unwrap() as usize {
                    let sum: f32 = (0..4)
                        .map(|k| {
                            f32::from_le_bytes(
                                bin[off + (v * 4 + k) * 4..][..4].try_into().unwrap(),
                            )
                        })
                        .sum();
                    assert!((sum - 1.0).abs() < 1e-5, "{}: weight sum {sum}", cand.id);
                }
            }
        }
    }
    eprintln!("checked {skinned} skinned primitives");
}
