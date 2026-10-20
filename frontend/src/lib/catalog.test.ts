import { describe, expect, test } from "bun:test";
import {
	attrLabel,
	dialogShortLabel,
	entityVariant,
	facilityTint,
	hasGenderedText,
	humanizeCode,
	isCatalogKind,
	itemTypeRank,
	locateDialog,
	richToPlain,
	sortAttrs,
	tagIcon,
	titleCaseShouting,
} from "@/lib/catalog";

describe("richToPlain", () => {
	test("empty values", () => {
		expect(richToPlain(null)).toBe("");
		expect(richToPlain(undefined)).toBe("");
		expect(richToPlain("")).toBe("");
	});
	test("joins segments and strips <p=...> tags", () => {
		expect(richToPlain([{ text: "a" }, { text: "b" }] as never)).toBe("ab");
		expect(richToPlain('deal <p="x">10</p> damage')).toBe("deal 10 damage");
	});
});

describe("attrLabel / sortAttrs", () => {
	test("known and derived labels", () => {
		expect(attrLabel("MaxHp")).toBe("HP");
		expect(attrLabel("FireDamageIncrease")).toBe("Fire Damage Increase");
		expect(attrLabel("CrystDamageIncrease")).toBe("Cryo Damage Increase");
	});
	test("primary attributes first, rest alphabetical", () => {
		expect(sortAttrs(["Zed", "Def", "Alpha", "MaxHp", "Atk"])).toEqual([
			"MaxHp",
			"Atk",
			"Def",
			"Alpha",
			"Zed",
		]);
	});
	test("does not mutate input", () => {
		const input = ["Def", "MaxHp"];
		sortAttrs(input);
		expect(input).toEqual(["Def", "MaxHp"]);
	});
});

describe("tagIcon", () => {
	test("known tags map to sprite paths", () => {
		expect(tagIcon({ kind: "profession", code: "GUARD" })).toEndWith(
			"icon_profession_0.png",
		);
		expect(tagIcon({ kind: "element", code: "Fire" })).toEndWith(
			"icon_charattrtype_fire.png",
		);
		expect(tagIcon({ kind: "weaponType", code: "Sword" })).toEndWith(
			"icon_weapontype_01.png",
		);
	});
	test("unknown codes and kinds return null", () => {
		expect(tagIcon({ kind: "profession", code: "NOPE" })).toBeNull();
		expect(tagIcon({ kind: "element", code: "NOPE" })).toBeNull();
		expect(tagIcon({ kind: "weaponType", code: "NOPE" })).toBeNull();
		expect(tagIcon({ kind: "faction", code: "x" })).toBeNull();
	});
});

describe("entity helpers", () => {
	test("entityVariant", () => {
		expect(entityVariant("chr_0001_endminm")).toBe("m");
		expect(entityVariant("chr_0001_endminf")).toBe("f");
		expect(entityVariant("chr_0002_other")).toBeNull();
	});
	test("hasGenderedText", () => {
		expect(hasGenderedText("chr_0001_endminm")).toBe(true);
		expect(hasGenderedText("story_ENDMIN_intro")).toBe(true);
		expect(hasGenderedText("chr_0002")).toBe(false);
	});
	test("isCatalogKind", () => {
		expect(isCatalogKind("operators")).toBe(true);
		expect(isCatalogKind("nope")).toBe(false);
	});
});

describe("text helpers", () => {
	test("titleCaseShouting", () => {
		expect(titleCaseShouting("ENDFIELD INDUSTRIES")).toBe(
			"Endfield Industries",
		);
		expect(titleCaseShouting("A-B/C (D)")).toBe("A-B/C (D)");
		expect(titleCaseShouting("Already Mixed")).toBe("Already Mixed");
		expect(titleCaseShouting("123")).toBe("123");
	});
	test("humanizeCode", () => {
		expect(humanizeCode("MachineCrafter")).toBe("Machine Crafter");
		expect(humanizeCode("battle_cannon_1")).toBe("Battle cannon 1");
		expect(humanizeCode("a--b")).toBe("A b");
	});
});

describe("facilityTint", () => {
	test("by type prefix with a fallback", () => {
		expect(facilityTint("BattleTurret")).toBe("#e5484d");
		expect(facilityTint("PowerStation")).toBe("#f5a524");
		expect(facilityTint("Unknown")).toBe("#7c8494");
		expect(facilityTint(undefined)).toBe("#7c8494");
	});
});

describe("itemTypeRank", () => {
	test("ordering: listed first, unlisted by count, then last group", () => {
		expect(itemTypeRank("Material")).toBe(0);
		expect(itemTypeRank("Weapon")).toBeGreaterThan(itemTypeRank("Material"));
		const counts = new Map([["Foo", 100]]);
		expect(itemTypeRank("Foo", counts)).toBe(900);
		expect(itemTypeRank("Foo", counts)).toBeLessThan(
			itemTypeRank("Bar", counts),
		);
		expect(itemTypeRank("PhotoSticker")).toBe(10_000);
		expect(itemTypeRank(undefined)).toBe(5000);
		expect(itemTypeRank("Bar")).toBeLessThan(itemTypeRank("PhotoSticker"));
	});
});

describe("dialog helpers", () => {
	test("dialogShortLabel", () => {
		expect(dialogShortLabel("dlg_a1m2_3", "a1m2")).toBe("3");
		expect(dialogShortLabel("dlg_a_b_3", "a/b")).toBe("3");
		expect(dialogShortLabel("dlg_zzz", "a1m2")).toBe("zzz");
	});
	test("locateDialog", () => {
		const group = { id: "g", dialogs: ["d1", "d2"] };
		const category = { id: "mission", groups: [group] };
		const list = { categories: [category] } as never;
		const hit = locateDialog(list, "d2");
		expect(hit?.index).toBe(1);
		expect(hit?.group as unknown).toBe(group);
		expect(locateDialog(list, "zz")).toBeNull();
		expect(locateDialog(undefined, "d1")).toBeNull();
	});
});
