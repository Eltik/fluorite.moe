use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Fixture, body_json, get};
use crate::api::catalog::assets::{
    MeshIndex, RigIndex, clips_index, lights_file, model_parts, split_lod,
};
use crate::api::story::{natural_cmp, replace_non_finite};

const UI: &str = "textures/beyond/dynamicassets/gameplay/ui/sprites";
const CHEN_NAME: i64 = 3128157701878862541;
const CHEN_EN_CV: i64 = -6130144450069822276;

fn t(id: i64) -> Value {
    json!({ "id": id, "text": "" })
}

/// The base fixture plus a miniature game-data tree for the catalog and story endpoints.
pub fn catalog_fixture() -> Fixture {
    let fx = Fixture::new();
    let out = fx.output();
    let write = |rel: &str, bytes: &[u8]| {
        let p = out.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    let table = |name: &str, v: Value| {
        write(
            &format!("decrypted/Table/TableCfg/{name}.json"),
            serde_json::to_string(&v).unwrap().as_bytes(),
        )
    };

    let row = |stage: u32, lv: f64, hp: f64, atk: f64| {
        json!({ "breakStage": stage, "Attribute": { "attrs": [
            { "attrType": "Level", "attrValue": lv },
            { "attrType": "MaxHp", "attrValue": hp },
            { "attrType": "Atk", "attrValue": atk },
            { "attrType": "Weight", "attrValue": 1.0 },
        ]}})
    };
    table(
        "CharacterTable",
        json!({
            "chr_0005_chen": {
                "charId": "chr_0005_chen", "name": t(CHEN_NAME), "engName": "Chen Qianyu",
                "rarity": 5, "profession": "GUARD", "charTypeId": "Physical", "weaponType": "Sword",
                "department": "ENDFIELD INDUSTRIES", "mainAttrType": "Agi", "subAttrType": "Str",
                "defaultWeaponId": "wpn_sword_0003",
                "cvName": { "EngCVName": t(CHEN_EN_CV), "JapCVName": t(0) },
                "charBattleTagIds": ["tag_03"],
                "attributes": [row(0, 1.0, 500.0, 30.0), row(0, 2.0, 550.0, 33.0), row(1, 2.0, 560.0, 33.0)],
                "profileRecord": [{ "id": "chr_0005_chen_1", "recordTitle": t(140), "recordDesc": t(141), "unlockType": "DefaultUnlock" }],
                "profileVoice": [{ "id": "chr_0005_chen_1", "voId": "chr_0005_chen_chrbark_squad_01", "voiceTitle": t(142), "voiceDesc": t(143) }]
            },
            "chr_0004_pelica": {
                "charId": "chr_0004_pelica", "name": t(150), "engName": "Perlica", "rarity": 5,
                "profession": "CASTER", "charTypeId": "Pulse", "weaponType": "Wand"
            },
            "chr_9000_endmin": { "charId": "chr_9000_endmin", "name": t(151), "rarity": 6 }
        }),
    );
    table(
        "CharGrowthTable",
        json!({ "chr_0005_chen": {
            "skillGroupMap": { "chr_0005_chen_NormalSkill": {
                "skillGroupId": "chr_0005_chen_NormalSkill", "skillGroupType": "NormalSkill",
                "name": t(100), "desc": t(101), "icon": "icon_skill_chen_01",
                "skillIdList": ["chr_0005_chen_normal_skill"]
            }},
            "talentNodeMap": {
                "chr_0005_chen_passive_skill_0_1": { "nodeId": "chr_0005_chen_passive_skill_0_1", "nodeType": "PassiveSkill",
                    "passiveSkillNodeInfo": { "name": t(144), "iconId": "icon_talent_chen_01", "level": 1, "breakStage": 1,
                        "talentEffectId": "chr_0005_chen_talent_1_1" },
                    "requiredItem": [{ "id": "item_gold", "count": 2400 }] },
                "chr_0005_chen_1": { "nodeId": "chr_0005_chen_1", "nodeType": "Attr",
                    "attributeNodeInfo": { "title": t(145), "desc": t(146), "breakStage": 1,
                        "attributeModifiers": [{ "attrType": "Agi", "attrValue": 10.0, "modifierType": "BaseAddition" }] } }
            },
            "charBreakCostMap": { "charBreak20": { "nodeId": "charBreak20", "nodeType": "CharBreak", "breakStage": 1,
                "name": t(147), "description": t(148), "requiredItem": [{ "id": "item_char_break_stage_1_2", "count": 8 }] } }
        }}),
    );
    let lvl = |level: u32, poise: f64, scale: f64| {
        json!({ "level": level, "skillName": t(0), "description": t(0), "costType": "Atb", "costValue": 100.0,
            "blackboard": [{ "key": "atk_scale", "value": scale, "valueStr": "" }, { "key": "poise", "value": poise, "valueStr": "" }],
            "subDescDataList": [{ "name": t(102), "desc": format!("{}%", (scale * 100.0).round()) }] })
    };
    table(
        "SkillPatchTable",
        json!({
            "chr_0005_chen_normal_skill": { "SkillPatchDataBundle": [lvl(1, 10.0, 1.690000057220459), lvl(2, 12.0, 1.86)] },
            "wpn_attr_agi_low": { "SkillPatchDataBundle": [
                { "level": 1, "skillName": t(113), "description": t(114), "blackboard": [{ "key": "agi", "value": 10.0, "valueStr": "" }] }
            ]}
        }),
    );
    table(
        "PotentialTalentEffectTable",
        json!({
            "chr_0005_chen_talent_1_1": { "desc": t(103), "dataList": [{ "modifyType": "AddBuff",
                "attachBuff": { "blackboard": [{ "key": "atk_up", "value": 0.03999999910593033, "valueStr": "" }] } }] },
            "chr_0005_chen_potential_4": { "desc": t(104), "dataList": [{ "modifyType": "ChangeSkillParam",
                "skillParamModifier": { "paramType": "CostValue", "paramValue": 0.8500000238418579, "modifyType": "Multiply" } }] }
        }),
    );
    table(
        "CharacterPotentialTable",
        json!({ "chr_0005_chen": { "potentialUnlockBundle": [
            { "level": 4, "name": t(105), "potentialEffectId": "chr_0005_chen_potential_4" }
        ]}}),
    );
    table(
        "CharProfessionTable",
        json!({ "0": { "profession": "GUARD", "name": t(106), "iconId": "icon_profession_0" } }),
    );
    table(
        "CharTypeTable",
        json!({ "Physical": { "charTypeId": "Physical", "name": t(107), "color": "888888", "icon": "icon_charattrtype_physical" } }),
    );
    table("CharBattleTagTable", json!({ "tag_03": t(149) }));
    table(
        "TextTable",
        json!({ "LUA_WEAPON_TYPE_1": t(108), "t_name": t(120) }),
    );
    table(
        "RichTextStyleTable",
        json!({ "ba.pd": { "id": "ba.pd", "preDef": ["<color=#e3c19a>", "<color=#000000>"], "postDef": ["</color>"] } }),
    );
    table(
        "HyperlinkTextTable",
        json!({ "ba.noguard": { "id": "ba.noguard", "name": t(109), "desc": t(110),
            "iconPath": "TermIcon/icon_term_ba_noguard", "richTextId": "ba.pd" } }),
    );
    table(
        "WeaponBasicTable",
        json!({ "wpn_sword_0003": { "weaponId": "wpn_sword_0003", "engName": t(111), "weaponDesc": t(112),
            "rarity": 4, "weaponType": "Sword", "maxLv": 90, "levelTemplateId": "curve",
            "weaponSkillList": ["wpn_attr_agi_low"] } }),
    );
    table(
        "WeaponUpgradeTemplateTable",
        json!({ "curve": { "list": [{ "weaponLv": 1, "baseAtk": 30 }, { "weaponLv": 2, "baseAtk": 33 }] } }),
    );
    table(
        "EnemyTemplateDisplayInfoTable",
        json!({ "eny_0007_mimicw": { "templateId": "eny_0007_mimicw", "name": t(115), "nickname": t(0),
            "description": t(0), "displayType": "HighLevel", "abilityDescIds": ["eny_0007_mimicw_ability_1"], "tags": [] } }),
    );
    table(
        "EnemyTable",
        json!({
            "eny_0007_mimicw": { "enemyId": "eny_0007_mimicw", "templateId": "eny_0007_mimicw", "attrTemplateId": "eny_0007_mimicw" },
            "eny_0007_mimicw_dungeon": { "enemyId": "eny_0007_mimicw_dungeon", "templateId": "eny_0007_mimicw", "attrTemplateId": "eny_0007_mimicw" }
        }),
    );
    table(
        "EnemyAttributeTemplateTable",
        json!({ "eny_0007_mimicw": {
            "levelDependentAttributes": [
                { "attrs": [{ "attrType": "Level", "attrValue": 1.0 }, { "attrType": "MaxHp", "attrValue": 692.0 }, { "attrType": "Def", "attrValue": 100.0 }] },
                { "attrs": [{ "attrType": "Level", "attrValue": 2.0 }, { "attrType": "MaxHp", "attrValue": 790.0 }, { "attrType": "Def", "attrValue": 100.0 }] }
            ],
            "levelIndependentAttributes": { "attrs": [{ "attrType": "MaxPoise", "attrValue": 160.0 }] },
            "naturalResistance": 30.0, "maxResilience": 65.0
        }}),
    );
    table(
        "EnemyAbilityDescTable",
        json!({ "eny_0007_mimicw_ability_1": { "name": t(0), "description": t(116) } }),
    );
    table("DisplayEnemyTypeTable", json!({ "3": { "name": t(117) } }));
    table(
        "ItemTable",
        json!({
            "item_gold": { "id": "item_gold", "name": t(118), "desc": t(0), "rarity": 4, "type": "Gold",
                "iconId": "item_gold", "obtainWayIds": ["item_obtain_explore"] },
            "item_char_break_stage_1_2": { "id": "item_char_break_stage_1_2", "name": t(119), "rarity": 3,
                "type": "CharGradeUp", "iconId": "item_char_break_stage_1_2" },
            "wpn_sword_0003": { "id": "wpn_sword_0003", "name": t(111), "rarity": 4, "type": "Weapon", "iconId": "wpn_sword_0003" }
        }),
    );
    table(
        "ItemTypeTable",
        json!({ "0": { "itemType": "Gold", "name": t(121) }, "1": { "itemType": "CharGradeUp", "name": t(122) } }),
    );
    table(
        "SystemJumpTable",
        json!({ "item_obtain_explore": { "desc": t(123), "bindSystem": "None" } }),
    );
    table(
        "NpcTable",
        json!({ "npc_9108_qinjc_g01_i002": { "npcId": "npc_9108_qinjc_g01_i002", "name": t(124), "title": t(125),
            "faction": t(126), "dataKey": "npc_spl_qinjc_01", "npcGroupId": "npc_spl_qinjc_01_g01" } }),
    );
    table(
        "FactoryBuildingTable",
        json!({ "furnance_1": { "id": "furnance_1", "name": t(127), "desc": t(128), "type": "MachineCrafter",
            "iconOnPanel": "icon_port_furnance_1", "powerConsume": 5, "needPower": true, "bandwidth": 2,
            "range": { "width": 3, "depth": 3, "height": 4 }, "inputPorts": [{}, {}], "outputPorts": [{}] } }),
    );
    table(
        "DialogTextTable",
        json!({
            "dlg_t_1_001": { "actorName": t(130), "actorNameId": "pelica", "dialogText": t(131), "emotionType": "Calm" },
            "dlg_t_1_002": { "actorName": t(130), "actorNameId": "pelica", "dialogText": t(132) }
        }),
    );
    table(
        "DialogOptionTable",
        json!({ "option_dlg_t_1_1_001": { "optionText": t(133) }, "option_dlg_t_1_1_002": { "optionText": t(134) } }),
    );
    table(
        "ActorImageTable",
        json!({ "chr_0004_pelica": { "avatarPath": "icon_chr_0004_pelica" } }),
    );

    let en_pairs: Vec<(i64, &str)> = vec![
        (CHEN_NAME, "Chen Qianyu"),
        (CHEN_EN_CV, "Rebecca Hanssen"),
        (100, "Ascending Strike"),
        (
            101,
            "Deals <@ba.pd>Physical DMG</> and {poise:0} Stagger to <#ba.noguard>Vulnerable</> foes.",
        ),
        (102, "DMG Multiplier"),
        (103, "ATK +{atk_up:0%}."),
        (104, "Ultimate Energy cost -{1-costvalue:0%}."),
        (105, "Improvised Chi Xiao"),
        (106, "Guard"),
        (107, "Physical"),
        (108, "Sword"),
        (109, "Vulnerable"),
        (110, "Takes extra <@ba.pd>Physical</> effects."),
        (111, "Tarr 11"),
        (112, "A plain sword."),
        (113, "Agility [S]"),
        (114, "Agility <@ba.vup>+{agi}</>"),
        (115, "Tunneling Nidwyrm"),
        (116, "Avoid the poisonous spit."),
        (117, "Advanced"),
        (118, "T-Creds"),
        (119, "Protodisk"),
        (120, "Test Mission"),
        (121, ""),
        (122, "Operator Material"),
        (123, "Exploring and Gathering"),
        (124, "Qin Jiangchi"),
        (125, "Hub Base Director"),
        (126, "Ally"),
        (127, "Refining Unit"),
        (128, "<b>Power</b>: <@ba.pd>5</>"),
        (130, "Perlica"),
        (131, "{F}She's here.{M}He's here."),
        (132, "It's alright."),
        (133, "Sorry."),
        (134, "Hello."),
        (140, "BASIC INFO"),
        (141, "CODENAME: Chen Qianyu"),
        (142, "Operation Prep 1"),
        (143, "My blades are always ready."),
        (144, "Slashing Edge"),
        (145, "Skirmisher"),
        (146, "Operator Agility +10."),
        (147, "Promotion I"),
        (148, "Raise the level cap to 40"),
        (149, "Lift"),
        (150, "Perlica"),
    ];
    let en: serde_json::Map<String, Value> = en_pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), Value::from(v)))
        .collect();
    let en = Value::Object(en);
    let cn = json!({ CHEN_NAME.to_string(): "陈千语", "106": "近卫", "121": "货币", "101": "造成<@ba.pd>物理伤害</>和{poise:0}点失衡。" });
    table("I18nTextTable_EN", en);
    table("I18nTextTable_CN", cn);

    for p in [
        "charicon/icon_chr_0005_chen.png",
        "charicon/icon_chr_0004_pelica.png",
        "characterportrait/chr_0005_chen.png",
        "skillicon/icon_skill_chen_01.png",
        "skillicon/icon_talent_chen_01.png",
        "charprofessionicon/icon_profession_0.png",
        "itemicon/item_gold.png",
        "itemicon/wpn_sword_0003.png",
        "gachaweapon/wpn_sword_0003.png",
        "monstericon/eny_0007_mimicw.png",
        "factory/buildingpanelicon/icon_port_furnance_1.png",
        "termicon/icon_term_ba_noguard.png",
    ] {
        write(&format!("{UI}/{p}"), b"png");
    }
    let mesh = "meshes/beyond/arts/entity";
    for p in [
        "actor/girl/chen/models/S_actor_chen_body_01_lod0.glb",
        "actor/girl/chen/models/S_actor_chen_body_01_lod1.glb",
        "actor/girl/chen/models/S_actor_chen_cloth_01_lod2.glb",
        "actor/girl/chen/models/S_actor_chen_cloth_01_lod1.glb",
        "actor/girl/chen/animations/S_actor_chen_jnt_lod0.glb",
        "monster/elite/mimicw/models/S_monster_mimicw_01_lod0.glb",
        "wpn/sword/wpn_sword_0003/models/S_wpn_sword_0003_01_lod0.glb",
        "npc/major/girl/qinjc/models/S_actor_qinjc_body_01_lod0.glb",
    ] {
        write(&format!("{mesh}/{p}"), b"glTF");
    }
    write(
        "models/index.json",
        br#"{"models":[{"kind":"operators","id":"chr_0005_chen","path":"models/operators/chr_0005_chen.glb"}]}"#,
    );
    write("models/operators/chr_0005_chen.glb", b"glTF");
    write(
        "models/clips/operators/chr_0005_chen/index.json",
        br#"[{"file":"a_actor_chen_idle.glb","name":"idle","clip":"A_actor_chen_idle","source":"3c","duration":2.0,"samples":121,"fps":60.0,"loop":true,"humanoid":true,"tracks":300,"matched":300,"size":4,"rootMotion":false}]"#,
    );
    write(
        "models/clips/operators/chr_0005_chen/a_actor_chen_idle.glb",
        b"glTF",
    );
    write(
        "models/lights/chr_0005_chen.json",
        br#"{"id":"chr_0005_chen","volume":{"multiplier":1.0},"lights":[{"name":"RimLight_1","npr":"rim"}],"camera":{"position":[0.0,0.998,3.5],"target":[0.0,1.225,0.0],"fov":20.0}}"#,
    );
    write(
        "models/clips/operators/chr_0005_chen/menu.json",
        br#"{"format":1,"default":"Overview.OverviewIdle","states":{"Overview.OverviewIdle":{"clip":"a_actor_chen_idle.glb","length":2.0,"loop":true,"exit":null}},"any":[],"moments":[]}"#,
    );
    write(
        "models/charinfo/weapons/chr_0005_chen.json",
        br#"{"format":1,"id":"chr_0005_chen","weapon":"wpn_sword_0015","rule":"signature","mounts":[{"node":"wep_R","scale":1.0}],"clips":{}}"#,
    );
    write(
        "models/charinfo/deco/chr_0005_chen/index.json",
        br#"{"format":1,"id":"chr_0005_chen","items":[{"n":1,"file":"1.glb","mount":"inner","kind":"deco"}],"clips":{}}"#,
    );

    // Dialog: line -> (spliced ExActor) -> option(2) -> line / finish. Contains a bare
    // `Infinity` like Unity's serializer emits.
    write(
        "gamedata/beyond/dynamicassets/gameplay/dialog/json/mission/t/dlg_t_1.json",
        br#"{"nodes":[
            {"$id":"0","$type":"Beyond.Gameplay.DialogTreeTrunkNode","_actorNodeData":{"mfTrunkActionData":{"_trunkId":"dlg_t_1_001"},"curve":{"inTangent":Infinity}}},
            {"$id":"5","$type":"Beyond.Gameplay.DialogTreeExActorNode"},
            {"$id":"1","$type":"Beyond.Gameplay.DialogTreeOptionNode","_normalOptions":[{"_optionId":"option_dlg_t_1_1_001"},{"_optionId":"option_dlg_t_1_1_002"}]},
            {"$id":"2","$type":"Beyond.Gameplay.DialogTreeTrunkNode","_actorNodeData":{"mfTrunkActionData":{"_trunkId":"dlg_t_1_002"}}},
            {"$id":"3","$type":"Beyond.Gameplay.DialogTreeFinishNode"}
        ],"connections":[
            {"_sourceNode":{"$ref":"0"},"_targetNode":{"$ref":"5"}},
            {"_sourceNode":{"$ref":"5"},"_targetNode":{"$ref":"1"}},
            {"_sourceNode":{"$ref":"1"},"_targetNode":{"$ref":"2"}},
            {"_sourceNode":{"$ref":"1"},"_targetNode":{"$ref":"3"}},
            {"_sourceNode":{"$ref":"2"},"_targetNode":{"$ref":"3"}}
        ]}"#,
    );
    write(
        "gamedata/beyond/dynamicassets/gameplay/dialog/json/npc/dlg_npc_0004_pelica_gift.json",
        br#"{"nodes":[{"$id":"0","$type":"Beyond.Gameplay.DialogTreeFinishNode"}],"connections":[]}"#,
    );
    fx
}

fn text_of(segs: &Value) -> String {
    segs.as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap_or(""))
        .collect()
}

/// Every JSON number must be exactly representable in a JS double.
fn assert_js_safe(v: &Value) {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                assert!(i.unsigned_abs() <= 1 << 53, "unsafe integer {i}");
            }
            if let Some(u) = n.as_u64() {
                assert!(u <= 1 << 53, "unsafe integer {u}");
            }
        }
        Value::Array(a) => a.iter().for_each(assert_js_safe),
        Value::Object(o) => o.values().for_each(assert_js_safe),
        _ => {}
    }
}

#[test]
fn lod_and_parts() {
    assert_eq!(
        split_lod("S_actor_chen_body_01_lod2.glb"),
        Some(("S_actor_chen_body_01", 2))
    );
    assert_eq!(split_lod("S_wpn_x_01.glb"), Some(("S_wpn_x_01", 0)));
    assert_eq!(split_lod("a.png"), None);
    let fx = catalog_fixture();
    let meshes = fx.output().join("meshes");
    let ix = MeshIndex::build(&meshes);
    let (dir, code) = ix.operator("chr_0005_chen").unwrap();
    assert_eq!(dir, "beyond/arts/entity/actor/girl/chen");
    let parts = model_parts(&meshes, dir, &code);
    let summary: Vec<_> = parts.iter().map(|p| (p.name.as_str(), p.lod)).collect();
    assert_eq!(summary, vec![("body_01", 0), ("cloth_01", 1)]);
    assert!(
        ix.enemy("eny_0007_mimicw2").is_some(),
        "digit-stripped code"
    );
    assert_eq!(ix.npc("npc_spl_qinjc_01").unwrap().1, "qinjc");
    assert!(
        ix.weapon("wpn_sword_0003_ink").is_some(),
        "variant falls back to base id"
    );
}

#[test]
fn story_helpers() {
    assert_eq!(
        natural_cmp("dlg_a1m2_1", "dlg_a1m10_1"),
        std::cmp::Ordering::Less
    );
    assert_eq!(natural_cmp("x10", "x9"), std::cmp::Ordering::Greater);
    assert_eq!(natural_cmp("x09", "x9"), std::cmp::Ordering::Equal);
    let fixed = replace_non_finite(br#"{"a":Infinity,"b":-Infinity,"c":NaN,"s":"NaN Infinity"}"#);
    let v: Value = serde_json::from_slice(&fixed).unwrap();
    assert_eq!(
        v,
        json!({"a": null, "b": null, "c": null, "s": "NaN Infinity"})
    );
}

#[tokio::test]
async fn catalog_index_and_lists() {
    let fx = catalog_fixture();
    let app = fx.app();

    let v = body_json(get(&app, "/api/catalog").await).await;
    let counts: Vec<(String, u64)> = v["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["kind"].as_str().unwrap().to_string(),
                k["count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        vec![
            ("operators".into(), 2),
            ("weapons".into(), 1),
            ("enemies".into(), 1),
            ("items".into(), 3),
            ("npcs".into(), 1),
            ("facilities".into(), 1)
        ]
    );

    let v = body_json(get(&app, "/api/catalog/operators").await).await;
    assert_eq!(v["kind"], "operators");
    assert_eq!(v["total"], 2);
    let chen = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "chr_0005_chen")
        .unwrap();
    assert_eq!(chen["name"], "Chen Qianyu");
    assert_eq!(chen["rarity"], 5);
    assert_eq!(
        chen["icon"],
        "beyond/dynamicassets/gameplay/ui/sprites/charicon/icon_chr_0005_chen.png"
    );
    assert_eq!(chen["hasModel"], true);
    assert_eq!(
        chen["tags"],
        json!([
            {"kind": "profession", "code": "GUARD", "name": "Guard"},
            {"kind": "element", "code": "Physical", "name": "Physical"},
            {"kind": "weaponType", "code": "Sword", "name": "Sword"}
        ])
    );
    let pelica = &v["items"][0];
    assert_eq!(pelica["id"], "chr_0004_pelica");
    assert_eq!(pelica["hasModel"], false);

    let v = body_json(get(&app, "/api/catalog/operators?lang=cn").await).await;
    assert_eq!(v["lang"], "CN");
    assert_eq!(v["items"][1]["name"], "陈千语");
    // Missing in CN → EN fallback.
    assert_eq!(v["items"][0]["name"], "Perlica");

    // Items: search, type filter, pagination, facets.
    let v = body_json(get(&app, "/api/catalog/items?q=cred").await).await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["id"], "item_gold");
    // EN type name is empty → falls back to CN.
    assert_eq!(v["items"][0]["tags"][0]["name"], "货币");
    let v = body_json(get(&app, "/api/catalog/items?type=CharGradeUp").await).await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["name"], "Protodisk");
    let v = body_json(get(&app, "/api/catalog/items?limit=1&offset=1").await).await;
    assert_eq!(v["total"], 3);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["id"], "item_gold");
    assert_eq!(v["types"].as_array().unwrap().len(), 3);

    assert_eq!(
        get(&app, "/api/catalog/nope").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/catalog/items?lang=../x").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/api/catalog/items?lang=DE").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn operator_detail() {
    let fx = catalog_fixture();
    let app = fx.app();
    let res = get(&app, "/api/catalog/operators/chr_0005_chen").await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_js_safe(&v);
    assert_eq!(v["name"], "Chen Qianyu");
    assert_eq!(v["profession"]["name"], "Guard");
    assert_eq!(v["element"]["color"], "#888888");
    assert_eq!(v["weaponType"]["name"], "Sword");
    assert_eq!(v["voiceActors"], json!({"en": "Rebecca Hanssen"}));
    assert_eq!(
        v["defaultWeapon"],
        json!({"id": "wpn_sword_0003", "name": "Tarr 11"})
    );
    assert_eq!(v["battleTags"][0]["name"], "Lift");

    let attrs = &v["attributes"];
    assert_eq!(attrs["level"], json!([1, 2, 2]));
    assert_eq!(attrs["breakStage"], json!([0, 0, 1]));
    assert_eq!(attrs["series"]["MaxHp"], json!([500, 550, 560]));
    assert_eq!(attrs["constants"]["Weight"], 1);

    let skill = &v["skills"][0];
    assert_eq!(skill["name"], "Ascending Strike");
    assert_eq!(
        skill["icon"],
        "beyond/dynamicassets/gameplay/ui/sprites/skillicon/icon_skill_chen_01.png"
    );
    let l1 = &skill["levels"][0];
    assert_eq!(
        l1["description"],
        json!([
            {"text": "Deals "},
            {"text": "Physical DMG", "style": "ba.pd", "color": "#e3c19a"},
            {"text": " and 10 Stagger to "},
            {"text": "Vulnerable", "term": "ba.noguard", "color": "#e3c19a"},
            {"text": " foes."}
        ])
    );
    assert_eq!(l1["blackboard"], json!({"atk_scale": 1.69, "poise": 10}));
    assert_eq!(
        l1["stats"],
        json!([{"name": "DMG Multiplier", "value": "169%"}])
    );
    assert_eq!(l1["cost"], json!({"type": "Atb", "value": 100}));
    assert!(text_of(&skill["levels"][1]["description"]).contains("12 Stagger"));

    let term = &v["terms"]["ba.noguard"];
    assert_eq!(term["name"], "Vulnerable");
    assert_eq!(
        term["icon"],
        "beyond/dynamicassets/gameplay/ui/sprites/termicon/icon_term_ba_noguard.png"
    );
    assert_eq!(
        text_of(&term["description"]),
        "Takes extra Physical effects."
    );

    assert_eq!(text_of(&v["talents"][0]["description"]), "ATK +4%.");
    assert_eq!(
        v["talents"][0]["icon"],
        "beyond/dynamicassets/gameplay/ui/sprites/skillicon/icon_talent_chen_01.png"
    );
    assert_eq!(
        text_of(&v["potentials"][0]["description"]),
        "Ultimate Energy cost -15%."
    );
    assert_eq!(v["attributeNodes"][0]["modifiers"][0]["value"], 10);
    assert_eq!(v["promotions"][0]["cost"][0]["count"], 8);
    assert_eq!(v["profile"][0]["title"], "BASIC INFO");
    assert_eq!(v["voiceLines"][0]["voId"], "chr_0005_chen_chrbark_squad_01");

    assert_eq!(
        v["art"],
        json!({
            "icon": "beyond/dynamicassets/gameplay/ui/sprites/charicon/icon_chr_0005_chen.png",
            "portrait": "beyond/dynamicassets/gameplay/ui/sprites/characterportrait/chr_0005_chen.png"
        })
    );
    assert_eq!(v["model"]["dir"], "beyond/arts/entity/actor/girl/chen");
    assert_eq!(
        v["model"]["parts"],
        json!([
            {"name": "body_01", "path": "beyond/arts/entity/actor/girl/chen/models/S_actor_chen_body_01_lod0.glb", "lod": 0},
            {"name": "cloth_01", "path": "beyond/arts/entity/actor/girl/chen/models/S_actor_chen_cloth_01_lod1.glb", "lod": 1}
        ])
    );
    assert_eq!(v["model"]["rigged"], "operators/chr_0005_chen.glb");
    let rigged = get(&app, "/files/models/operators/chr_0005_chen.glb").await;
    assert_eq!(rigged.status(), StatusCode::OK);
    // Per-clip animation GLBs: the index sits next to the clips under the models root.
    assert_eq!(
        v["model"]["clips"],
        "clips/operators/chr_0005_chen/index.json"
    );
    let clips = body_json(
        get(
            &app,
            "/files/models/clips/operators/chr_0005_chen/index.json",
        )
        .await,
    )
    .await;
    assert_eq!(clips[0]["file"], "a_actor_chen_idle.glb");
    let clip = get(
        &app,
        "/files/models/clips/operators/chr_0005_chen/a_actor_chen_idle.glb",
    )
    .await;
    assert_eq!(clip.status(), StatusCode::OK);
    // Menu lighting written by the exporter, also under the models root.
    assert_eq!(v["model"]["lights"], "lights/chr_0005_chen.json");
    let lights = body_json(get(&app, "/files/models/lights/chr_0005_chen.json").await).await;
    assert_eq!(lights["lights"][0]["npr"], "rim");
    // Character-screen data written by `endfield-models charinfo`.
    assert_eq!(
        v["model"]["menu"],
        "clips/operators/chr_0005_chen/menu.json"
    );
    assert_eq!(v["model"]["weapon"], "charinfo/weapons/chr_0005_chen.json");
    assert_eq!(v["model"]["deco"], "charinfo/deco/chr_0005_chen/index.json");
    assert_eq!(
        v["model"]["camera"],
        json!({"position": [0.0, 0.998, 3.5], "target": [0.0, 1.225, 0.0], "fov": 20.0})
    );
    let menu = body_json(
        get(
            &app,
            "/files/models/clips/operators/chr_0005_chen/menu.json",
        )
        .await,
    )
    .await;
    assert_eq!(menu["default"], "Overview.OverviewIdle");

    // CN: localized description, profession from CN, name from CN.
    let v = body_json(get(&app, "/api/catalog/operators/chr_0005_chen?lang=CN").await).await;
    assert_eq!(v["name"], "陈千语");
    assert_eq!(v["profession"]["name"], "近卫");
    assert_eq!(
        text_of(&v["skills"][0]["levels"][0]["description"]),
        "造成物理伤害和10点失衡。"
    );

    assert_eq!(
        get(&app, "/api/catalog/operators/chr_9999_nobody")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/catalog/operators/chr_0005_chen?gender=x")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn other_details() {
    let fx = catalog_fixture();
    let app = fx.app();

    let v = body_json(get(&app, "/api/catalog/weapons/wpn_sword_0003").await).await;
    assert_eq!(v["name"], "Tarr 11");
    assert_eq!(v["atk"], json!({"level": [1, 2], "baseAtk": [30, 33]}));
    assert_eq!(v["skills"][0]["name"], "Agility [S]");
    assert_eq!(
        text_of(&v["skills"][0]["levels"][0]["description"]),
        "Agility +10"
    );
    assert_eq!(
        v["skills"][0]["levels"][0]["description"][1]["style"],
        "ba.vup"
    );
    assert_eq!(
        v["art"]["gacha"],
        "beyond/dynamicassets/gameplay/ui/sprites/gachaweapon/wpn_sword_0003.png"
    );
    assert_eq!(v["model"]["parts"][0]["name"], "01");

    let v = body_json(get(&app, "/api/catalog/enemies/eny_0007_mimicw").await).await;
    assert_eq!(v["name"], "Tunneling Nidwyrm");
    assert_eq!(
        v["displayType"],
        json!({"code": "HighLevel", "name": "Advanced"})
    );
    assert_eq!(
        text_of(&v["abilities"][0]["description"]),
        "Avoid the poisonous spit."
    );
    let ids: Vec<&str> = v["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["eny_0007_mimicw", "eny_0007_mimicw_dungeon"]);
    let tpl = &v["attributeTemplates"]["eny_0007_mimicw"];
    assert_eq!(tpl["levels"]["series"]["MaxHp"], json!([692, 790]));
    assert_eq!(tpl["levels"]["constants"]["Def"], 100);
    assert_eq!(tpl["resistances"]["natural"], 30);
    assert_eq!(tpl["fixed"]["MaxPoise"], 160);
    assert_eq!(v["model"]["parts"][0]["name"], "01");

    let v = body_json(get(&app, "/api/catalog/items/item_gold").await).await;
    assert_eq!(v["name"], "T-Creds");
    assert_eq!(v["obtainWays"][0]["description"], "Exploring and Gathering");
    assert_eq!(
        v["art"]["icon"],
        "beyond/dynamicassets/gameplay/ui/sprites/itemicon/item_gold.png"
    );
    let v = body_json(get(&app, "/api/catalog/items/wpn_sword_0003").await).await;
    assert_eq!(
        v["related"],
        json!({"kind": "weapons", "id": "wpn_sword_0003"})
    );

    let v = body_json(get(&app, "/api/catalog/npcs/npc_9108_qinjc_g01_i002").await).await;
    assert_eq!(v["title"], "Hub Base Director");
    assert_eq!(v["faction"], "Ally");
    assert_eq!(v["model"]["dir"], "beyond/arts/entity/npc/major/girl/qinjc");

    let v = body_json(get(&app, "/api/catalog/facilities/furnance_1").await).await;
    assert_eq!(v["name"], "Refining Unit");
    assert_eq!(v["description"][0], json!({"text": "Power", "bold": true}));
    assert_eq!(v["range"], json!({"width": 3, "depth": 3, "height": 4}));
    assert_eq!(
        (v["inputPorts"].clone(), v["outputPorts"].clone()),
        (json!(2), json!(1))
    );
}

#[tokio::test]
async fn story_endpoints() {
    let fx = catalog_fixture();
    let app = fx.app();

    let v = body_json(get(&app, "/api/story").await).await;
    assert_eq!(v["total"], 2);
    assert_eq!(
        v["categories"],
        json!([
            {"id": "mission", "count": 1, "groups": [{"id": "t", "title": "Test Mission", "dialogs": ["dlg_t_1"]}]},
            {"id": "npc", "count": 1, "groups": [{"id": "", "dialogs": ["dlg_npc_0004_pelica_gift"]}]}
        ])
    );

    let res = get(&app, "/api/story/dlg_t_1").await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["title"], "Test Mission");
    assert_eq!(v["start"], "0");
    let nodes = v["nodes"].as_array().unwrap();
    let order: Vec<(&str, &str)> = nodes
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n["kind"].as_str().unwrap()))
        .collect();
    // The ExActor node "5" is spliced out.
    assert_eq!(
        order,
        vec![
            ("0", "line"),
            ("1", "option"),
            ("2", "line"),
            ("3", "finish")
        ]
    );
    assert_eq!(nodes[0]["next"], json!(["1"]));
    assert_eq!(
        nodes[0]["speaker"],
        json!({"name": "Perlica", "id": "pelica",
            "icon": "beyond/dynamicassets/gameplay/ui/sprites/charicon/icon_chr_0004_pelica.png"})
    );
    assert_eq!(text_of(&nodes[0]["text"]), "She's here.");
    assert_eq!(text_of(&nodes[0]["altText"]), "He's here.");
    assert_eq!(
        nodes[1]["options"],
        json!([
            {"id": "option_dlg_t_1_1_001", "text": [{"text": "Sorry."}], "next": "2"},
            {"id": "option_dlg_t_1_1_002", "text": [{"text": "Hello."}], "next": "3"}
        ])
    );
    assert_eq!(nodes[1]["next"], json!(["2", "3"]));

    let v = body_json(get(&app, "/api/story/dlg_t_1?gender=m").await).await;
    assert_eq!(text_of(&v["nodes"][0]["text"]), "He's here.");
    assert_eq!(text_of(&v["nodes"][0]["altText"]), "She's here.");

    assert_eq!(
        get(&app, "/api/story/dlg_missing").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/story/..%2F..%2Fsecret").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn models_root_is_browsable() {
    let fx = catalog_fixture();
    let app = fx.app();
    let v = body_json(get(&app, "/api/tree/models").await).await;
    let names: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["charinfo", "clips", "lights", "operators", "index.json"]
    );
}

/// Catalog ids resolve to rigged variant prefabs when no exact GLB exists.
#[test]
fn rig_index_falls_back_to_variant_prefabs() {
    let rig = RigIndex {
        by_stem: [
            ("eny_0054_hsmino_chrdg", "enemy/eny_0054_hsmino_chrdg.glb"),
            ("eny_0054_hsmino1", "enemy/eny_0054_hsmino1.glb"),
            ("eny_0113_jzogre", "enemy/eny_0113_jzogre.glb"),
            (
                "wpn_pistol_0009_refined",
                "weapon/wpn_pistol_0009_refined.glb",
            ),
            ("wpn_sword_00091", "weapon/wpn_sword_00091.glb"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        stamp: Some(std::time::SystemTime::UNIX_EPOCH),
    };
    let out = std::env::temp_dir().join("fluorite-rig-variant-test");
    let find = |kind: &str, id: &str| rig.find(&out, kind, id);
    assert_eq!(
        find("enemies", "eny_0113_jzogre").as_deref(),
        Some("enemy/eny_0113_jzogre.glb")
    );
    // Digit variants win over `_` variants.
    assert_eq!(
        find("enemies", "eny_0054_hsmino").as_deref(),
        Some("enemy/eny_0054_hsmino1.glb")
    );
    assert_eq!(
        find("weapons", "wpn_pistol_0009").as_deref(),
        Some("weapon/wpn_pistol_0009_refined.glb")
    );
    // A numeric id never claims a longer number.
    assert_eq!(find("weapons", "wpn_sword_0009"), None);
    assert_eq!(find("enemies", "eny_0055_hscrane"), None);
    // No index, no rigged model.
    assert_eq!(
        RigIndex::default().find(&out, "enemies", "eny_0113_jzogre"),
        None
    );
}

#[tokio::test]
async fn has_model_follows_the_rig_index() {
    let fx = catalog_fixture();
    let app = fx.app();
    let pelica = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "chr_0004_pelica")
            .unwrap()["hasModel"]
            .clone()
    };
    let v = body_json(get(&app, "/api/catalog/operators").await).await;
    assert_eq!(pelica(&v), false);

    let models = fx.output().join("models");
    std::fs::write(models.join("operators/chr_0004_pelica.glb"), b"glTF").unwrap();
    let index = models.join("index.json");
    std::fs::write(
        &index,
        br#"{"models":[
            {"path":"models/operators/chr_0005_chen.glb"},
            {"path":"models/operators/chr_0004_pelica.glb"}
        ]}"#,
    )
    .unwrap();
    std::fs::File::options()
        .append(true)
        .open(&index)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let v = body_json(get(&app, "/api/catalog/operators").await).await;
    assert_eq!(pelica(&v), true);
}

/// Kinds only see their own folder (operators and NPC variants share stems), and NPCs
/// resolve through `npc_chr_` / `npc_eny_` data keys.
#[test]
fn rig_index_scopes_kinds_and_maps_npc_data_keys() {
    let out = std::env::temp_dir().join(format!("fluorite-rig-npc-{}", std::process::id()));
    let models = out.join("models");
    for rel in [
        "operator/chr_0005_chen.glb",
        "npc/chr_0005_chen.glb",
        "operator/chr_0023_antal.glb",
        "enemy/eny_0021_agmelee.glb",
    ] {
        let path = models.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"glTF").unwrap();
    }
    let rig = RigIndex {
        by_stem: [("chr_0005_chen", "operator/chr_0005_chen.glb")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        stamp: Some(std::time::SystemTime::UNIX_EPOCH),
    };
    assert_eq!(
        rig.find(&out, "operators", "chr_0005_chen").as_deref(),
        Some("operator/chr_0005_chen.glb")
    );
    assert_eq!(
        rig.find(&out, "npcs", "chr_0005_chen").as_deref(),
        Some("npc/chr_0005_chen.glb")
    );
    assert_eq!(
        rig.find(&out, "enemies", "eny_0021_agmelee").as_deref(),
        Some("enemy/eny_0021_agmelee.glb")
    );
    // NPC rows: an NPC-mode prefab first, then the operator, then enemies via `npc_eny_`.
    assert_eq!(
        rig.find_npc(&out, "npc_0001_chen_g01_i001", "npc_chr_0005_chen")
            .as_deref(),
        Some("npc/chr_0005_chen.glb")
    );
    assert_eq!(
        rig.find_npc(&out, "npc_x", "npc_chr_0023_antal").as_deref(),
        Some("operator/chr_0023_antal.glb")
    );
    assert_eq!(
        rig.find_npc(&out, "npc_agmelee_1", "npc_eny_0021_agmelee")
            .as_deref(),
        Some("enemy/eny_0021_agmelee.glb")
    );
    assert_eq!(
        rig.find_npc(&out, "dajiang", "npc_gentleman_riverbandits_a_01"),
        None
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// The clip index is reported only when `endfield-models clips` wrote a non-empty one.
#[test]
fn clips_index_needs_a_non_empty_index() {
    let out = std::env::temp_dir().join(format!("fluorite-clips-{}", std::process::id()));
    let dir = out.join("models/clips/enemy");
    std::fs::create_dir_all(dir.join("eny_a")).unwrap();
    std::fs::create_dir_all(dir.join("eny_b")).unwrap();
    std::fs::write(dir.join("eny_a/index.json"), br#"[{"file":"x.glb"}]"#).unwrap();
    std::fs::write(dir.join("eny_b/index.json"), b"[]").unwrap();
    assert_eq!(
        clips_index(&out, Some("enemy/eny_a.glb")).as_deref(),
        Some("clips/enemy/eny_a/index.json")
    );
    assert_eq!(clips_index(&out, Some("enemy/eny_b.glb")), None);
    assert_eq!(clips_index(&out, Some("enemy/eny_c.glb")), None);
    assert_eq!(clips_index(&out, None), None);
    let _ = std::fs::remove_dir_all(&out);
}

/// Light data is reported only when the exporter wrote `lights/<id>.json`.
#[test]
fn lights_file_needs_an_exported_file() {
    let out = std::env::temp_dir().join(format!("fluorite-lights-{}", std::process::id()));
    std::fs::create_dir_all(out.join("models/lights")).unwrap();
    std::fs::write(out.join("models/lights/chr_a.json"), b"{}").unwrap();
    assert_eq!(
        lights_file(&out, Some("operator/chr_a.glb")).as_deref(),
        Some("lights/chr_a.json")
    );
    assert_eq!(lights_file(&out, Some("operator/chr_b.glb")), None);
    assert_eq!(lights_file(&out, None), None);
    let _ = std::fs::remove_dir_all(&out);
}
