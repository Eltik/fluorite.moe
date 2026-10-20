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
    ([-1.5, 0.0, 2.0], [0.0, H, 0.0, H], [1.0, 2.0, 0.5]),
    ([0.0, 3.0, 0.0], [-H, 0.0, 0.0, H], [1.0, 1.0, 1.0]),
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
        gltf::check_document(&doc, bin).is_empty(),
        "{:?}",
        gltf::check_document(&doc, bin)
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
    let errors = gltf::check_document(&doc, bin);
    assert!(errors.iter().any(|e| e.contains("IBM count")), "{errors:?}");

    let (mut doc, glb) = skinned_document(2);
    let (_, bin) = gltf::parse_glb(&glb).unwrap();
    doc["meshes"][0]["primitives"][0]["material"] = json!(9);
    doc["nodes"][1]["children"] = json!([0, 0]);
    let errors = gltf::check_document(&doc, bin);
    assert!(
        errors.iter().any(|e| e.contains("missing material")),
        "{errors:?}"
    );
    assert!(errors.iter().any(|e| e.contains("2 parents")), "{errors:?}");
    assert!(!gltf::check_document(&doc, &bin[..8]).is_empty());
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
            resource_sizes: BTreeMap::new(),
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
        assert!(gltf::check_document(&doc, bin).is_empty());
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

mod anim {
    use crate::extraction::rig::anim::{self, DecodedClip, TrackBinding};
    use crate::extraction::rig::gltf::{self, Builder};
    use serde_json::{Value, json};

    fn binding(path: u32, attribute: i64, type_id: i64) -> Value {
        json!({"path": path, "attribute": attribute, "typeID": type_id, "customType": 0, "isPPtrCurve": 0})
    }

    #[test]
    fn crc32_matches_ieee_check_value() {
        assert_eq!(anim::crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(anim::crc32(b""), 0);
    }

    /// ACL transform track `i` is the `i`-th distinct Transform path, in binding order;
    /// non-Transform bindings (Animator floats) are not tracks.
    #[test]
    fn bindings_group_into_tracks_in_first_seen_order() {
        let bindings = vec![
            binding(10, 1, 4),
            binding(20, 1, 4),
            binding(10, 2, 4),
            binding(20, 2, 4),
            binding(20, 3, 4),
            binding(99, 0, 95),
        ];
        let tracks = anim::transform_bindings(&bindings);
        assert_eq!(
            tracks,
            vec![
                TrackBinding {
                    path_hash: 10,
                    position: true,
                    rotation: true,
                    scale: false
                },
                TrackBinding {
                    path_hash: 20,
                    position: true,
                    rotation: true,
                    scale: true
                },
            ]
        );
    }

    #[test]
    fn quaternion_keys_stay_in_one_hemisphere() {
        let mut keys = [
            [0.0, 0.0, 0.1, 0.995],
            [0.0, 0.0, -0.2, -0.98],
            [0.0, 0.0, 0.3, 0.954],
        ];
        anim::make_continuous(&mut keys);
        assert!(keys[1][3] > 0.0 && keys[2][3] > 0.0);
        assert!(anim::is_constant(&[[1.0f32, 2.0], [1.0, 2.0 + 1e-7]], 1e-6));
        assert!(!anim::is_constant(&[[1.0f32, 2.0], [1.0, 2.1]], 1e-6));
    }

    /// Node paths below the animator root hash like Unity binding paths ("" is the root).
    #[test]
    fn node_paths_hash_like_unity() {
        let nodes = vec![
            json!({"name": "root", "children": [1]}),
            json!({"name": "Bip001", "children": [2]}),
            json!({"name": "Bip001 Pelvis"}),
        ];
        let map = anim::node_path_hashes(&nodes, 0);
        assert_eq!(map[&anim::crc32(b"")], 0);
        assert_eq!(map[&anim::crc32(b"Bip001")], 1);
        assert_eq!(map[&anim::crc32(b"Bip001/Bip001 Pelvis")], 2);
    }

    /// Channels are converted to glTF space, constant curves collapse to one key, and the
    /// document stays structurally valid.
    #[test]
    fn decoded_clip_becomes_valid_gltf_animation() {
        let tracks = vec![
            TrackBinding {
                path_hash: anim::crc32(b"a"),
                position: true,
                rotation: true,
                scale: false,
            },
            TrackBinding {
                path_hash: anim::crc32(b"missing"),
                position: true,
                rotation: false,
                scale: false,
            },
        ];
        let samples_per_track = |s: usize, moving: bool| {
            let angle = if moving { s as f32 * 0.1 } else { 0.0 };
            // rotation xyzw (about Y), translation xyz, scale xyz
            vec![
                0.0,
                (angle / 2.0).sin(),
                0.0,
                (angle / 2.0).cos(),
                1.0,
                2.0,
                3.0,
                1.0,
                1.0,
                1.0,
            ]
        };
        let mut samples = Vec::new();
        for s in 0..3 {
            samples.extend(samples_per_track(s, true));
            samples.extend(samples_per_track(s, false));
        }
        let clip = DecodedClip {
            name: "test".into(),
            sample_rate: 30.0,
            num_samples: 3,
            tracks,
            samples,
        };
        let mut b = Builder::default();
        b.nodes.push(json!({"name": "root", "children": [1]}));
        b.nodes.push(json!({"name": "a"}));
        let nodes = anim::node_path_hashes(&b.nodes, 0);
        let mut animations = Vec::new();
        let stats = anim::add_animation(&mut b, &mut animations, &clip, &nodes);
        assert_eq!(
            (stats.matched_tracks, stats.unmatched_tracks, stats.channels),
            (1, 1, 2)
        );
        assert!((stats.duration - 2.0 / 30.0).abs() < 1e-6);
        b.animations = animations;
        let (doc, glb) = b.finish(vec![0], json!({}));
        let (_, bin) = gltf::parse_glb(&glb).unwrap();
        assert!(gltf::check_document(&doc, bin).is_empty());
        let anim = &doc["animations"][0];
        let accessor = |i: &Value| doc["accessors"][i.as_u64().unwrap() as usize].clone();
        let rotation = anim["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["target"]["path"] == "rotation")
            .unwrap();
        let translation = anim["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["target"]["path"] == "translation")
            .unwrap();
        let sampler = |c: &Value| anim["samplers"][c["sampler"].as_u64().unwrap() as usize].clone();
        assert_eq!(accessor(&sampler(rotation)["output"])["count"], 3);
        // Constant translation keeps a single key; X is mirrored.
        let t_out = accessor(&sampler(translation)["output"]);
        assert_eq!(t_out["count"], 1);
        let view = &doc["bufferViews"][t_out["bufferView"].as_u64().unwrap() as usize];
        let off = view["byteOffset"].as_u64().unwrap() as usize;
        let x = f32::from_le_bytes(bin[off..off + 4].try_into().unwrap());
        assert_eq!(x, -1.0);
    }

    #[cfg(feature = "acl")]
    #[test]
    fn acl_rejects_garbage_buffers() {
        use crate::extraction::rig::acl;
        assert!(acl::transforms(&[0u8; 64]).is_err());
        assert!(acl::info(&[]).is_err());
    }

    /// Decodes a real clip's `TransformBufferData` (dumped to a file) with the vendored ACL.
    /// `RIG_ACL_FIXTURE=<file> cargo test --features acl -- --ignored acl_fixture`
    #[cfg(feature = "acl")]
    #[test]
    #[ignore = "needs RIG_ACL_FIXTURE pointing at a dumped TransformBufferData"]
    fn acl_fixture_decodes_unit_quaternions() {
        use crate::extraction::rig::acl;
        let Ok(path) = std::env::var("RIG_ACL_FIXTURE") else {
            return;
        };
        let data = std::fs::read(path).unwrap();
        let (info, out) = acl::transforms(&data).unwrap();
        assert_eq!(info.track_type, 12);
        assert_eq!(
            out.len(),
            info.num_tracks as usize * info.num_samples as usize * acl::TRANSFORM_STRIDE
        );
        for q in out.as_chunks::<{ acl::TRANSFORM_STRIDE }>().0 {
            let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "non-unit quaternion {len}");
        }
    }
}

mod review_fixes {
    use super::skinned_document;
    use crate::extraction::rig::acl;
    use crate::extraction::rig::anim::{self, DecodedClip, TrackBinding};
    use crate::extraction::rig::entity::{claim_output, kind_of, merge_index};
    use crate::extraction::rig::gltf::{self, Builder};
    use crate::extraction::rig::store::{self, CabRecord, FileLock};
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};

    /// NPC variants of operators are their own kind and never claim the operator's file.
    #[test]
    fn npc_postmodels_are_not_operators() {
        let npc = "assets/beyond/dynamicassets/gameplay/actors/postmodels/npc/chr_0023_antal_postmodel.prefab";
        let op = "assets/beyond/dynamicassets/gameplay/actors/postmodels/characters/chr_0023_antal_postmodel.prefab";
        assert_eq!(kind_of(npc), "npc");
        assert_eq!(kind_of(op), "operator");
    }

    #[test]
    fn output_paths_are_unique_per_run() {
        let mut claimed = HashMap::new();
        let a = claim_output(&mut claimed, "operator", "chr_0023_antal", "a/x.prefab");
        let again = claim_output(&mut claimed, "operator", "chr_0023_antal", "a/x.prefab");
        let b = claim_output(&mut claimed, "operator", "chr_0023_antal", "b/x.prefab");
        assert_eq!(a, "operator/chr_0023_antal.glb");
        assert_eq!(again, a);
        assert_ne!(b, a);
        assert!(b.starts_with("operator/chr_0023_antal__") && b.ends_with(".glb"));
    }

    /// Fresh entries replace old ones for the same container or the same file, and no two
    /// merged entries share a GLB.
    #[test]
    fn merged_index_never_shares_a_file() {
        let old = json!({"entries": [
            {"kind": "operator", "id": "chr_0023_antal", "container": "npc/antal", "glb": "operator/chr_0023_antal.glb"},
            {"kind": "enemy", "id": "eny_1", "container": "e1", "glb": "enemy/eny_1.glb"},
            {"kind": "enemy", "id": "eny_1b", "container": "e1b", "glb": "enemy/eny_1.glb"},
            {"kind": "weapon", "id": "wpn_1", "container": "w1", "glb": "weapon/wpn_1.glb"},
        ]});
        let fresh = vec![
            json!({"kind": "operator", "id": "chr_0023_antal", "container": "characters/antal", "glb": "operator/chr_0023_antal.glb"}),
            json!({"kind": "weapon", "id": "wpn_1", "container": "w1", "glb": "weapon/wpn_1.glb"}),
        ];
        let merged = merge_index(Some(&old), fresh);
        let glbs: Vec<&str> = merged.iter().filter_map(|e| e["glb"].as_str()).collect();
        let mut unique = glbs.clone();
        unique.dedup();
        assert_eq!(glbs.len(), 3, "{merged:?}");
        assert_eq!(unique.len(), glbs.len());
        let antal = merged.iter().find(|e| e["id"] == "chr_0023_antal").unwrap();
        assert_eq!(antal["container"], "characters/antal");
    }

    #[test]
    fn temp_paths_are_unique_and_hidden() {
        let path = std::path::Path::new("/store/res/CAB-x.resS");
        let a = store::temp_path(path);
        let b = store::temp_path(path);
        let c = store::temp_path(std::path::Path::new("/store/res/CAB-x.resource"));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.parent(), path.parent());
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".CAB-x.resS.")
        );
    }

    #[test]
    fn file_lock_is_exclusive_and_released() {
        let dir = std::env::temp_dir().join(format!("fluorite-rig-lock-{}", std::process::id()));
        let path = dir.join("x.lock");
        let lock = FileLock::try_acquire(&path).unwrap().expect("free lock");
        assert!(FileLock::try_acquire(&path).unwrap().is_none());
        drop(lock);
        assert!(FileLock::try_acquire(&path).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rec(
        cab: &str,
        chunk: &str,
        classes: &[i32],
        container: &str,
        resources: &[&str],
    ) -> CabRecord {
        CabRecord {
            cab: cab.into(),
            chunk: chunk.into(),
            sf_size: 0,
            resources: resources.iter().map(|r| r.to_string()).collect(),
            resource_sizes: BTreeMap::new(),
            classes: classes.iter().map(|c| (*c, 1)).collect(),
            containers: vec![(container.into(), 0, 1)],
            externals: vec![],
        }
    }

    /// A CAB listed by a stale (irrelevant-looking) record and a newer relevant one is kept.
    #[test]
    fn prune_keeps_files_any_record_needs() {
        let entity = "assets/beyond/arts/entity/actor/girl/chen/models/a.fbx";
        let scene = "assets/beyond/arts/environment/x.mat";
        let records = vec![
            rec("CAB-shared", "OLD", &[114], scene, &["CAB-shared.resS"]),
            rec("CAB-shared", "NEW", &[43], entity, &["CAB-shared.resS"]),
            rec("CAB-scene", "NEW", &[43], scene, &["CAB-scene.resS"]),
        ];
        let (sf, res) = store::prune_plan(&records);
        assert!(!sf.contains("CAB-shared") && !res.contains("CAB-shared.resS"));
        assert!(sf.contains("CAB-scene") && res.contains("CAB-scene.resS"));
    }

    #[test]
    fn acl_output_size_is_checked() {
        assert_eq!(
            acl::output_len(373, 151, acl::TRANSFORM_STRIDE),
            Ok(563_230)
        );
        assert!(acl::output_len(u32::MAX, u32::MAX, acl::TRANSFORM_STRIDE).is_err());
        assert!(acl::output_len(1 << 16, 1 << 11, 1).is_err());
    }

    #[test]
    fn clips_without_usable_timing_are_rejected_without_panicking() {
        assert!(anim::validate_timing(0, 60.0).is_err());
        assert!(anim::validate_timing(10, 0.0).is_err());
        assert!(anim::validate_timing(10, f32::NAN).is_err());
        assert!(anim::validate_timing(1, 0.0).is_ok());
        assert!(anim::validate_timing(151, 60.0).is_ok());
        let tracks = vec![TrackBinding {
            path_hash: 0,
            position: true,
            rotation: true,
            scale: false,
        }];
        for (num_samples, rate, samples) in [
            (0, 60.0, vec![]),
            (3, 60.0, vec![0.0; 10]),
            (2, 0.0, vec![0.0; 20]),
        ] {
            let clip = DecodedClip {
                name: "bad".into(),
                sample_rate: rate,
                num_samples,
                tracks: tracks.clone(),
                samples,
            };
            let mut b = Builder::default();
            b.nodes.push(json!({"name": "root"}));
            let nodes = anim::node_path_hashes(&b.nodes, 0);
            let mut animations = Vec::new();
            let stats = anim::add_animation(&mut b, &mut animations, &clip, &nodes);
            assert_eq!(stats.channels, 0);
            assert!(animations.is_empty());
        }
    }

    #[test]
    fn repeated_joints_are_rejected() {
        let (mut doc, glb) = skinned_document(2);
        let (_, bin) = gltf::parse_glb(&glb).unwrap();
        let root = doc["skins"][0]["joints"][0].clone();
        doc["skins"][0]["joints"] = json!([root, root]);
        let errors = gltf::check_document(&doc, bin);
        assert!(
            errors.iter().any(|e| e.contains("more than once")),
            "{errors:?}"
        );
    }

    #[test]
    fn joint_indices_beyond_the_skin_are_rejected() {
        let (mut doc, glb) = skinned_document(1);
        let (_, bin) = gltf::parse_glb(&glb).unwrap();
        // The mesh uses joint index 1; a one-joint skin can't serve it.
        let root = doc["skins"][0]["joints"][0].clone();
        doc["skins"][0]["joints"] = json!([root]);
        let errors = gltf::check_document(&doc, bin);
        assert!(
            errors.iter().any(|e| e.contains("JOINTS_0 index 1 >= 1")),
            "{errors:?}"
        );
    }
}

mod avatar_models {
    use crate::extraction::rig::avatar::Skeleton;
    use crate::extraction::rig::entity::{avatar_candidates, avatar_model_id};
    use crate::extraction::rig::store::CabRecord;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn avatar_model_ids_follow_npc_data_keys() {
        let path =
            "assets/beyond/arts/entity/npc/major/girl/qinjc/models/sk_npc_major_qinjc_01.fbx";
        assert_eq!(avatar_model_id(path), "spl_qinjc");
        assert_eq!(
            avatar_model_id("a/models/sk_npc_major_andrewhq_01.fbx"),
            "spl_andrewhq"
        );
        assert_eq!(avatar_model_id("a/models/sk_thing.fbx"), "thing");
    }

    #[test]
    fn avatar_candidates_need_avatar_meshes_and_no_game_objects() {
        let path =
            "assets/beyond/arts/entity/npc/major/girl/qinjc/models/sk_npc_major_qinjc_01.fbx";
        let rec = |cab: &str, classes: &[i32]| CabRecord {
            cab: cab.into(),
            chunk: "C".into(),
            sf_size: 0,
            resources: vec![],
            resource_sizes: BTreeMap::new(),
            classes: classes.iter().map(|c| (*c, 1)).collect(),
            containers: vec![(path.into(), 0, 1), (path.into(), 0, 2)],
            externals: vec![],
        };
        let found = avatar_candidates(
            &[rec("CAB-a", &[43, 90, 142]), rec("CAB-b", &[1, 4, 43, 90])],
            "npc/major",
        );
        assert_eq!(found.len(), 1);
        assert!(found[0].avatar);
        assert_eq!(found[0].kind, "npc");
        assert_eq!(found[0].id, "spl_qinjc");
        assert_eq!(
            found[0].sources,
            vec![("CAB-a".to_string(), 1), ("CAB-a".to_string(), 2)]
        );
    }

    fn pose(t: f64) -> serde_json::Value {
        json!({"t": {"x": 0.0, "y": t, "z": 0.0}, "q": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}, "s": {"x": 1.0, "y": 1.0, "z": 1.0}})
    }

    #[test]
    fn skeleton_reads_ids_parents_poses_and_tos_names() {
        let avatar = json!({
            "m_Avatar": {
                "m_AvatarSkeleton": {"data": {"m_ID": [0, 11, 22], "m_Node": [
                    {"m_ParentId": -1}, {"m_ParentId": 0}, {"m_ParentId": 1}
                ]}},
                "m_AvatarSkeletonPose": {"data": {"m_X": [pose(0.0), pose(1.0), pose(0.5)]}}
            },
            "m_TOS": {"0": "", "11": "Root", "22": "Root/Bip001"}
        });
        let s = Skeleton::from_avatar(&avatar).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!((s.parent(0), s.parent(2)), (None, Some(1)));
        assert_eq!(s.name(2), "Bip001");
        assert_eq!(s.local[1].0, [0.0, 1.0, 0.0]);
        // Children listed before their parents are rejected.
        let mut bad = avatar.clone();
        bad["m_Avatar"]["m_AvatarSkeleton"]["data"]["m_Node"][1]["m_ParentId"] = json!(2);
        assert!(Skeleton::from_avatar(&bad).is_err());
    }
}

#[test]
fn clips_for_other_skeletons_are_detected() {
    use crate::extraction::rig::anim::{self, DecodedClip, TrackBinding};
    let track = |h| TrackBinding {
        path_hash: h,
        position: false,
        rotation: true,
        scale: false,
    };
    let clip = DecodedClip {
        name: "x".into(),
        sample_rate: 30.0,
        num_samples: 1,
        tracks: vec![track(1), track(2), track(3), track(4)],
        samples: vec![],
    };
    let nodes: std::collections::HashMap<u32, usize> = [(1, 0)].into_iter().collect();
    assert!((anim::matched_fraction(&clip, &nodes) - 0.25).abs() < 1e-6);
}
