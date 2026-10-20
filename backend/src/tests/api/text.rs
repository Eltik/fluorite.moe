use std::collections::HashMap;
use std::sync::Arc;

use crate::api::catalog::text::{
    BbEntry, Blackboard, Gender, Markup, Segment, TermDef, TextRef, Texts, eval_expr,
    format_number, has_gender_variants, parse_markup, pick_gender, render_placeholders,
    strip_markup,
};

fn bb(pairs: &[(&str, f64)]) -> Blackboard {
    let rows: Vec<BbEntry> = pairs
        .iter()
        .map(|(k, v)| BbEntry {
            key: k.to_string(),
            value: *v,
            value_str: String::new(),
        })
        .collect();
    Blackboard::from_rows(&rows)
}

fn seg(text: &str) -> Segment {
    Segment {
        text: text.into(),
        ..Default::default()
    }
}

#[test]
fn number_formats_follow_csharp_patterns() {
    // f32-stored values as they appear in SkillPatchTable.
    assert_eq!(format_number(0.1599999964237213, "0%"), "16%");
    assert_eq!(format_number(0.1599999964237213, "0.0%"), "16.0%");
    assert_eq!(format_number(0.02500000037252903, "0.0%"), "2.5%");
    assert_eq!(format_number(0.024999998, "0.0%"), "2.5%");
    assert_eq!(format_number(0.005, "0%"), "1%"); // half away from zero
    assert_eq!(format_number(10.0, "0"), "10");
    assert_eq!(format_number(1.690000057220459, "0.00"), "1.69");
    assert_eq!(format_number(1.5, "0.##"), "1.5");
    assert_eq!(format_number(2.0, "0.##"), "2");
    assert_eq!(format_number(156.0, ""), "156");
    assert_eq!(format_number(0.10000000149011612, ""), "0.1");
    assert_eq!(format_number(-0.0001, "0"), "0");
    assert_eq!(format_number(-3.0, "0"), "-3");
    assert_eq!(format_number(0.0, "0.0"), "0.0");
}

#[test]
fn expressions() {
    let b = bb(&[
        ("costvalue", 0.85),
        ("spell_dmg_up2", 0.012),
        ("max_stack", 16.0),
        ("combo_cd_enhance", 0.1),
        ("atk_scale_3", 2.5),
    ]);
    assert!((eval_expr("1-costvalue", &b).unwrap() - 0.15).abs() < 1e-9);
    assert!((eval_expr("spell_dmg_up2*max_stack", &b).unwrap() - 0.192).abs() < 1e-9);
    assert!((eval_expr("-combo_cd_enhance", &b).unwrap() + 0.1).abs() < 1e-9);
    assert_eq!(eval_expr("(1 + atk_scale_3) * 2", &b), Some(7.0));
    assert_eq!(eval_expr("atk_scale_3/0", &b), None);
    assert_eq!(eval_expr("nope", &b), None);
    assert_eq!(eval_expr("1-", &b), None);
    assert_eq!(eval_expr("", &b), None);
}

#[test]
fn placeholders_render_or_stay_literal() {
    let mut b = bb(&[
        ("hp_up", 0.125),
        ("atk_up", 0.1599999964237213),
        ("duration", 20.0),
        ("costvalue", 0.85),
        ("spell_dmg_up2", 0.012),
        ("max_stack", 16.0),
        ("combo_cd_enhance", 0.1),
        ("atk_scale_3", 1.234),
    ]);
    b.strs.insert("str".into(), "Heat".into());
    let r = |s: &str| render_placeholders(s, &b);
    assert_eq!(r("HP <@ba.vup>+{hp_up:0.0%}</>"), "HP <@ba.vup>+12.5%</>");
    assert_eq!(r("ATK +{atk_up:0%} for {duration:0}s"), "ATK +16% for 20s");
    assert_eq!(r("cost -{1-costvalue:0%}"), "cost -15%");
    assert_eq!(r("{spell_dmg_up2*max_stack:0.0%}"), "19.2%");
    assert_eq!(r("CD {-combo_cd_enhance:0%}"), "CD -10%");
    assert_eq!(r("x{atk_scale_3:0.00}"), "x1.23");
    assert_eq!(r("{duration}s / {str}"), "20s / Heat");
    // Unknown names, gender markers, cross refs and stray braces are untouched.
    assert_eq!(r("{unknown:0%} {F}a{M}b"), "{unknown:0%} {F}a{M}b");
    assert_eq!(
        r("{buff_common_atk_buff_potion_1\\value:0%}"),
        "{buff_common_atk_buff_potion_1\\value:0%}"
    );
    assert_eq!(
        r("{e4m1d5-Representative of the UWST}"),
        "{e4m1d5-Representative of the UWST}"
    );
    assert_eq!(r("a { b {duration:0} }"), "a { b 20 }");
    assert_eq!(r("open {"), "open {");
}

#[test]
fn gender_variants() {
    let s = "{F}She's here.{M}He's here.";
    assert!(has_gender_variants(s));
    assert_eq!(pick_gender(s, Gender::Female), "She's here.");
    assert_eq!(pick_gender(s, Gender::Male), "He's here.");
    assert_eq!(pick_gender("{M}him{F}her", Gender::Female), "her");
    assert_eq!(pick_gender("{M}him{F}her", Gender::Male), "him");
    assert_eq!(pick_gender("plain", Gender::Male), "plain");
    assert_eq!(Gender::parse(Some("M")).unwrap(), Gender::Male);
    assert_eq!(Gender::parse(None).unwrap(), Gender::Female);
    assert!(Gender::parse(Some("x")).is_err());
}

fn markup() -> Markup {
    let mut m = Markup::default();
    m.styles.insert("ba.vup".into(), "#9eb7ff".into());
    m.styles.insert("ba.phy".into(), "#e3c19a".into());
    m.terms.insert(
        "ba.noguard".into(),
        TermDef {
            style: Some("ba.phy".into()),
            ..Default::default()
        },
    );
    m
}

#[test]
fn markup_becomes_segments() {
    let m = markup();
    let segs = parse_markup(
        "Deals <@ba.vup>+16%</> to <#ba.noguard>Vulnerable</> foes.\nNext",
        &m,
    );
    assert_eq!(
        segs,
        vec![
            seg("Deals "),
            Segment {
                text: "+16%".into(),
                style: Some("ba.vup".into()),
                color: Some("#9eb7ff".into()),
                ..Default::default()
            },
            seg(" to "),
            Segment {
                text: "Vulnerable".into(),
                term: Some("ba.noguard".into()),
                color: Some("#e3c19a".into()),
                ..Default::default()
            },
            seg(" foes.\nNext"),
        ]
    );

    // Unity tags: bold, explicit color, inline image and action glyphs.
    let segs = parse_markup(
        "<b>ATK</b>: <color=#f4690c>981</color> <image=\"KeyIcon/icon_key_alt\" scale=1.3> press <action=common_interact>",
        &m,
    );
    assert_eq!(segs[0].text, "ATK");
    assert!(segs[0].bold);
    assert_eq!(segs[1], seg(": "));
    assert_eq!(segs[2].color.as_deref(), Some("#f4690c"));
    assert_eq!(segs[2].text, "981");
    assert_eq!(segs[4].image.as_deref(), Some("KeyIcon/icon_key_alt"));
    assert_eq!(segs[6].action.as_deref(), Some("common_interact"));

    // Nested style inside a term keeps the term and takes the inner color.
    let segs = parse_markup("<#ba.noguard>a <@ba.vup>b</> c</>", &m);
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[1].term.as_deref(), Some("ba.noguard"));
    assert_eq!(segs[1].color.as_deref(), Some("#9eb7ff"));
    assert_eq!(segs[2].text, " c");
    assert_eq!(segs[2].style, None);

    // Unknown or broken tags stay literal; `<image>name</image>` form works.
    assert_eq!(
        parse_markup("a < b <weird> c", &m),
        vec![seg("a < b <weird> c")]
    );
    let segs = parse_markup("x<image>sns_emoji_001</image>y", &m);
    assert_eq!(segs[1].image.as_deref(), Some("sns_emoji_001"));
    assert_eq!(segs[2], seg("y"));
    assert_eq!(strip_markup("<@ba.vup>+20</> Agility"), "+20 Agility");
}

#[test]
fn text_refs_are_int64_strings() {
    #[derive(serde::Deserialize)]
    struct W {
        a: TextRef,
        b: TextRef,
        c: TextRef,
        d: TextRef,
    }
    let w: W = serde_json::from_str(
        r#"{"a":{"id":-6130144450069822276,"text":""},"b":{"id":"3128157701878862541"},
            "c":{"id":12316588523405729340},"d":{"text":""}}"#,
    )
    .unwrap();
    assert_eq!(w.a.key(), "-6130144450069822276");
    assert_eq!(w.b.id, 3128157701878862541);
    // Unsigned spellings wrap to the signed id the i18n table uses.
    assert_eq!(w.c.key(), (12316588523405729340u64 as i64).to_string());
    assert!(w.d.is_empty());
}

#[test]
fn texts_fall_back_to_en_then_cn() {
    let map = |pairs: &[(&str, &str)]| -> Arc<HashMap<String, String>> {
        Arc::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    };
    let jp = map(&[("1", "ペリカ"), ("2", "")]);
    let en = map(&[("1", "Perlica"), ("2", "Only EN")]);
    let cn = map(&[("3", "只有中文")]);
    let t = Texts::new(
        "JP".into(),
        Gender::Female,
        jp,
        vec![en, cn],
        Arc::new(markup()),
    );
    assert_eq!(t.plain(TextRef { id: 1 }), "ペリカ");
    assert_eq!(t.plain(TextRef { id: 2 }), "Only EN");
    assert_eq!(t.plain(TextRef { id: 3 }), "只有中文");
    assert_eq!(t.plain(TextRef { id: 4 }), "");
    assert_eq!(t.plain(TextRef { id: 0 }), "");
}
