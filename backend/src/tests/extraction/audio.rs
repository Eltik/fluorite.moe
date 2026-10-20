use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::extraction::audio::*;

// ── Fixtures ─────────────────────────────────────────────────────────────

/// A RIFF/WAVE PCM payload: 16-bit, `channels`, 48 kHz, `frames` frames of a ramp.
pub fn pcm_wav(channels: u16, frames: u32) -> Vec<u8> {
    let data_len = frames * u32::from(channels) * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes());
    v.extend_from_slice(&(48_000 * u32::from(channels) * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames * u32::from(channels) {
        v.extend_from_slice(&(((i % 200) as i16 - 100) * 100).to_le_bytes());
    }
    v
}

/// A Wwise Vorbis header stub (format 0xFFFF, extensible fmt with a sample count).
fn vorbis_head(samples: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    v.extend_from_slice(&0x42u32.to_le_bytes());
    let mut fmt = vec![0u8; 0x42];
    fmt[0..2].copy_from_slice(&0xFFFFu16.to_le_bytes());
    fmt[2..4].copy_from_slice(&2u16.to_le_bytes());
    fmt[4..8].copy_from_slice(&48_000u32.to_le_bytes());
    fmt[0x18..0x1C].copy_from_slice(&samples.to_le_bytes());
    v.extend_from_slice(&fmt);
    v
}

fn hirc_obj(ty: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![ty];
    v.extend_from_slice(&((payload.len() + 4) as u32).to_le_bytes());
    v.extend_from_slice(&id.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// Node base params with no effects: fx override/count, metadata override/count, bus, parent.
fn node_base(parent: u32) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0];
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&parent.to_le_bytes());
    v.extend_from_slice(&[0; 8]);
    v
}

/// A v150 bank: sound(media) → random container → actor-mixer; `Play` targets the mixer.
pub fn bank_bytes(bank_id: u32, sound: u32, media: u32, event: u32) -> Vec<u8> {
    let (container, mixer, action): (u32, u32, u32) = (0x0C0C_0001, 0x0A0A_0001, 0x0ACC_0001);
    let mut sound_pl = Vec::new();
    sound_pl.extend_from_slice(&0x0004_0001u32.to_le_bytes());
    sound_pl.push(2);
    sound_pl.extend_from_slice(&media.to_le_bytes());
    sound_pl.extend_from_slice(&0u32.to_le_bytes());
    sound_pl.push(0);
    sound_pl.extend_from_slice(&node_base(container));
    let mut action_pl = Vec::new();
    action_pl.extend_from_slice(&0x0403u16.to_le_bytes());
    action_pl.extend_from_slice(&mixer.to_le_bytes());
    action_pl.push(0);
    let mut event_pl = vec![1];
    event_pl.extend_from_slice(&action.to_le_bytes());
    let objs = [
        hirc_obj(2, sound, &sound_pl),
        hirc_obj(5, container, &node_base(mixer)),
        hirc_obj(7, mixer, &node_base(0)),
        hirc_obj(3, action, &action_pl),
        hirc_obj(4, event, &event_pl),
    ];
    let mut hirc = (objs.len() as u32).to_le_bytes().to_vec();
    for o in &objs {
        hirc.extend_from_slice(o);
    }
    let mut v = Vec::new();
    v.extend_from_slice(b"BKHD");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&150u32.to_le_bytes());
    v.extend_from_slice(&bank_id.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(b"HIRC");
    v.extend_from_slice(&(hirc.len() as u32).to_le_bytes());
    v.extend_from_slice(&hirc);
    v
}

fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fluorite-audio-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

// ── Hashes ───────────────────────────────────────────────────────────────

#[test]
fn short_ids_are_case_insensitive_fnv1() {
    // Bank id of the `chr_0019_karin_combat_attack04_sv` voice bank in default_banks.pck.
    assert_eq!(short_id("chr_0019_karin_combat_attack04_sv"), 3_203_543_145);
    assert_eq!(short_id("CHR_0019_Karin_combat_attack04_sv"), 3_203_543_145);
    assert_eq!(
        dialog_key("chr_0019_karin_combat_attack04_sv"),
        3_203_543_145u32 as i32
    );
}

#[test]
fn external_ids_hash_the_voice_path() {
    // Matched by duration against default_english_stream_0.pck.
    assert_eq!(
        external_id(
            "english",
            "v1d0/Narrating/CommonExtra/lady_30_01/au_dlg_commvo_lady_30_01_sigh_02.wem"
        ),
        12_553_709_830_414_657_592
    );
    assert_eq!(
        external_id("English", "V1D0/x.wem"),
        fnv1_64_lower("voice/english/v1d0/x.wem")
    );
}

// ── Packages ─────────────────────────────────────────────────────────────

#[test]
fn package_round_trip() {
    let wav = pcm_wav(1, 480);
    let bank = bank_bytes(77, 5, 1001, 900);
    let ext = b"external payload, any bytes".to_vec();
    let pck = build_package(
        &[(0, "sfx"), (1, "english")],
        &[
            PackageFile {
                kind: EntryKind::Bank,
                id: 77,
                lang_id: 0,
                data: &bank,
            },
            PackageFile {
                kind: EntryKind::Stream,
                id: 1001,
                lang_id: 0,
                data: &wav,
            },
            PackageFile {
                kind: EntryKind::External,
                id: u64::MAX - 3,
                lang_id: 0,
                data: &ext,
            },
        ],
    );
    assert_eq!(
        header_len(&pck).unwrap(),
        parse_table(&pck).unwrap().header_len()
    );
    let t = parse_table(&pck).unwrap();
    assert_eq!(t.languages, vec![(0, "sfx".into()), (1, "english".into())]);
    assert_eq!(t.voice_language(), Some("english"));
    assert_eq!(
        t.entries.iter().map(|e| (e.kind, e.id)).collect::<Vec<_>>(),
        vec![
            (EntryKind::Bank, 77),
            (EntryKind::Stream, 1001),
            (EntryKind::External, u64::MAX - 3)
        ]
    );

    let path = temp_file("round_trip.pck", &pck);
    let f = PckFile::open(&path).unwrap();
    assert_eq!(f.read_entry(f.find(1001).unwrap(), None).unwrap(), wav);
    assert_eq!(f.read_entry(f.find(77).unwrap(), None).unwrap(), bank);
    assert_eq!(
        f.read_entry(f.find(u64::MAX - 3).unwrap(), None).unwrap(),
        ext
    );
    // Partial reads decipher the same prefix.
    assert_eq!(
        f.read_entry(f.find(1001).unwrap(), Some(10)).unwrap(),
        wav[..10]
    );
    // The payload on disk is enciphered.
    let e = *f.find(1001).unwrap();
    assert_ne!(&pck[e.offset as usize..e.offset as usize + 4], b"RIFF");
}

#[test]
fn rejects_bad_headers() {
    assert!(header_len(b"AKPK\x10\0\0\0").is_err());
    assert!(header_len(b":)xD").is_err());
    assert!(header_len(b":)xD\x01\0\0\0").is_err());
    let mut pck = build_package(&[(0, "sfx")], &[]);
    pck.truncate(pck.len() - 4);
    assert!(parse_table(&pck).is_err());
}

#[test]
fn entries_past_the_end_fail_cleanly() {
    let wav = pcm_wav(1, 100);
    let mut pck = build_package(
        &[(0, "sfx")],
        &[PackageFile {
            kind: EntryKind::Stream,
            id: 9,
            lang_id: 0,
            data: &wav,
        }],
    );
    pck.truncate(pck.len() - 50);
    let f = PckFile::open(temp_file("short.pck", &pck)).unwrap();
    assert!(f.read_entry(f.find(9).unwrap(), None).is_err());
}

// ── Payloads ─────────────────────────────────────────────────────────────

#[test]
fn identifies_payloads() {
    match identify(&pcm_wav(2, 4800)) {
        Payload::Wem(w) => {
            assert_eq!((w.codec(), w.channels, w.sample_rate), ("pcm", 2, 48_000));
            assert_eq!(w.samples, Some(4800));
            assert!((w.duration().unwrap() - 0.1).abs() < 1e-9);
        }
        other => panic!("{other:?}"),
    }
    match identify(&vorbis_head(96_000)) {
        Payload::Wem(w) => {
            assert_eq!(w.codec(), "vorbis");
            assert_eq!(w.duration(), Some(2.0));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        identify(&bank_bytes(42, 1, 2, 3)),
        Payload::Bank {
            id: 42,
            version: 150
        }
    );
    assert_eq!(identify(b"OggS...."), Payload::Unknown);
    assert_eq!(codec_name(0x3041), "opus");
    assert_eq!(codec_name(0x1234), "unknown");
}

// ── Banks ────────────────────────────────────────────────────────────────

#[test]
fn bank_events_reach_media_through_the_hierarchy() {
    let b = parse_bank(&bank_bytes(42, 5, 1001, 900)).unwrap();
    assert_eq!((b.id, b.version), (42, 150));
    assert_eq!(b.sounds.len(), 1);
    assert_eq!(b.sounds[0].media_id, 1001);
    assert_eq!(b.sounds[0].parent, Some(0x0C0C_0001));
    assert_eq!(b.events, vec![(900, vec![0x0ACC_0001])]);

    let mut g = EventGraph::default();
    g.add(&b);
    assert!(g.has_media(1001));
    assert_eq!(g.events_for_media(1001), vec![900]);
    assert!(g.events_for_media(1002).is_empty());
    assert!(parse_bank(b"RIFF").is_err());
}

#[test]
fn scans_names_from_text() {
    let wanted: HashSet<u32> = [short_id("au_sfx_door_open"), short_id("play_bgm_01")].into();
    let mut out = HashMap::new();
    scan_names(
        br#"{"cue":"Au_SFX_Door_Open","x":"path/to/play_bgm_01.wem","y":"nope"}"#,
        &wanted,
        &mut out,
    );
    assert_eq!(out.len(), 2);
    assert_eq!(out[&short_id("au_sfx_door_open")], "au_sfx_door_open");
}

// ── Index ────────────────────────────────────────────────────────────────

#[test]
fn index_links_streams_events_and_dialog() {
    let wav = pcm_wav(1, 4800);
    let bank = bank_bytes(short_id("bnk_ui"), 5, 1001, short_id("play_ui_click"));
    let sfx = build_package(
        &[(0, "sfx")],
        &[
            PackageFile {
                kind: EntryKind::Bank,
                id: u64::from(short_id("bnk_ui")),
                lang_id: 0,
                data: &bank,
            },
            PackageFile {
                kind: EntryKind::Stream,
                id: 1001,
                lang_id: 0,
                data: &wav,
            },
        ],
    );
    let path = "v1d0/Characters/chr_0005_chen/chr_0005_chen_chrbark_squad_01.wem";
    let voice = build_package(
        &[(0, "sfx"), (1, "english")],
        &[PackageFile {
            kind: EntryKind::External,
            id: external_id("english", path),
            lang_id: 0,
            data: &wav,
        }],
    );
    let sfx = PckFile::open(temp_file("idx_sfx.pck", &sfx)).unwrap();
    let voice_table = parse_table(&voice).unwrap();

    let mut b = IndexBuilder::new();
    b.add_pack(PackInput {
        name: "sfx".into(),
        group: "Audio".into(),
        size: sfx.len,
        table: &sfx.table,
        local: Some(&sfx),
        remote: false,
    });
    b.add_pack(PackInput {
        name: "voice".into(),
        group: "AudioEnglish".into(),
        size: voice.len() as u64,
        table: &voice_table,
        local: None,
        remote: true,
    });
    b.add_dialog(vec![
        DialogInput {
            key: dialog_key("chr_0005_chen_chrbark_squad_01"),
            path: path.into(),
            vo_type: "Line".into(),
            speaker: "chr_0005_chen".into(),
            codec: 4,
            durations: vec![("chinese".into(), 1.5), ("english".into(), 1.25)],
        },
        DialogInput {
            key: 5,
            path: "v1d0/none.wem".into(),
            vo_type: "Line".into(),
            speaker: String::new(),
            codec: 20,
            durations: vec![],
        },
    ]);
    let wanted = b.wanted_names();
    let mut names = HashMap::new();
    scan_names(b"play_ui_click bnk_ui", &wanted, &mut names);
    let idx = b.finish(&names);

    let stream = idx.entries.iter().find(|e| e.id == 1001).unwrap();
    assert_eq!(stream.codec.as_deref(), Some("pcm"));
    assert_eq!(stream.duration, Some(0.1));
    assert_eq!(stream.events, vec![short_id("play_ui_click")]);
    assert_eq!(idx.event_names[&short_id("play_ui_click")], "play_ui_click");
    let bank = idx
        .entries
        .iter()
        .find(|e| e.kind == EntryKind::Bank)
        .unwrap();
    assert_eq!(bank.name.as_deref(), Some("bnk_ui"));

    let ext = idx
        .entries
        .iter()
        .find(|e| e.kind == EntryKind::External)
        .unwrap();
    assert_eq!(ext.lang, "english");
    assert_eq!(ext.name.as_deref(), Some("chr_0005_chen_chrbark_squad_01"));
    assert_eq!(ext.duration, Some(1.25));
    assert_eq!(ext.codec.as_deref(), Some("vorbis"));

    let c = &idx.coverage;
    assert_eq!(
        (c.streams, c.streams_with_sound, c.streams_with_event),
        (1, 1, 1)
    );
    assert_eq!((c.externals, c.externals_with_dialog), (1, 1));
    assert_eq!((c.dialog_lines, c.dialog_with_audio), (2, 1));
    assert_eq!((c.events, c.events_named, c.banks_named), (1, 1, 1));

    let l = idx.lookups();
    assert_eq!(
        l.by_dialog[&dialog_key("chr_0005_chen_chrbark_squad_01")].len(),
        1
    );
    // The cached form round-trips.
    let back: AudioIndex = serde_json::from_slice(&serde_json::to_vec(&idx).unwrap()).unwrap();
    assert_eq!(back.entries.len(), idx.entries.len());
}

// ── Real data (opt-in) ───────────────────────────────────────────────────

fn decrypted() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output/decrypted")
}

/// `cargo test --release --lib real_local_index -- --ignored --nocapture`
#[test]
#[ignore = "needs the extracted audio packages"]
fn real_local_index() {
    let mut files = Vec::new();
    for g in ["InitAudio", "Audio"] {
        for sub in ["Initial", "Main", "Audit"] {
            let dir = decrypted().join(g).join("Audio/PCK/Windows").join(sub);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for f in rd.flatten() {
                    files.push((g.to_string(), f.path()));
                }
            }
        }
    }
    let t = std::time::Instant::now();
    let opened: Vec<_> = files
        .iter()
        .map(|(g, p)| (g.clone(), PckFile::open(p).unwrap()))
        .collect();
    let mut b = IndexBuilder::new();
    for (g, f) in &opened {
        b.add_pack(PackInput {
            name: f.path.file_stem().unwrap().to_string_lossy().into(),
            group: g.clone(),
            size: f.len,
            table: &f.table,
            local: Some(f),
            remote: false,
        });
    }
    let wanted = b.wanted_names();
    let mut names = HashMap::new();
    for f in std::fs::read_dir(decrypted().join("Table/TableCfg"))
        .unwrap()
        .flatten()
    {
        let n = f.file_name().to_string_lossy().to_string();
        if n.ends_with(".json") && !n.starts_with("I18n") {
            scan_names(&std::fs::read(f.path()).unwrap(), &wanted, &mut names);
        }
    }
    let idx = b.finish(&names);
    println!("{:#?} in {:?}", idx.coverage, t.elapsed());
    assert!(idx.coverage.streams_with_sound * 100 >= idx.coverage.streams * 99);
}
