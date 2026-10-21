import { describe, expect, test } from "bun:test";
import {
	type DecoIndex,
	decoVisibility,
	nodeName,
	parseDecoIndex,
	parseWeaponInfo,
	sampleTrack,
	type WeaponInfo,
	weaponVisible,
} from "@/lib/props";

const WEAPON_RAW = {
	format: 1,
	id: "chr_0005_chen",
	weapon: "wpn_sword_0015",
	rule: "signature",
	type: "Sword",
	rarity: 6,
	glb: "weapon/wpn_sword_0015.glb",
	mounts: [{ node: "wep_R", scale: 1 }, { node: "", scale: 2 }, { node: "x" }],
	unresolvedMounts: [],
	clips: {
		"a_actor_chen_ui_overview_to_weapon.glb": [
			[0.3749, 1],
			[0, 0],
		],
		"a_actor_chen_ui_overview_weapon_loop.glb": [[0, 1]],
		"a_actor_chen_ui_weapon_to_overview.glb": [
			[0, 1],
			[1.0907, 0],
		],
		junk: "nope",
	},
	hideWeapon: ["a_actor_chen_interact_camera_01.glb"],
	wipe: {},
};

const weapon = parseWeaponInfo(WEAPON_RAW) as WeaponInfo;

describe("parseWeaponInfo", () => {
	test("keeps the mounts with a node and sorts the tracks", () => {
		expect(weapon.glb).toBe("weapon/wpn_sword_0015.glb");
		expect(weapon.mounts).toEqual([
			{ node: "wep_R", scale: 1 },
			{ node: "x", scale: 1 },
		]);
		expect(weapon.clips["a_actor_chen_ui_overview_to_weapon.glb"]).toEqual([
			[0, 0],
			[0.3749, 1],
		]);
		expect(weapon.clips.junk).toBeUndefined();
		expect(weapon.hideWeapon).toEqual(["a_actor_chen_interact_camera_01.glb"]);
	});

	test("tolerates a missing GLB", () => {
		expect(parseWeaponInfo({ id: "x", glb: null })?.glb).toBeNull();
		expect(parseWeaponInfo(null)).toBeNull();
	});
});

describe("sampleTrack", () => {
	test("steps at each key and holds the first value before the start", () => {
		const t: [number, number][] = [
			[0.5, 1],
			[2, 0],
		];
		expect(sampleTrack(t, 0)).toBe(1);
		expect(sampleTrack(t, 0.5)).toBe(1);
		expect(sampleTrack(t, 1.9)).toBe(1);
		expect(sampleTrack(t, 2)).toBe(0);
		expect(sampleTrack(t, 10)).toBe(0);
	});
});

describe("weaponVisible", () => {
	test("follows the clip's track", () => {
		const clip = "a_actor_chen_ui_overview_to_weapon.glb";
		expect(weaponVisible(weapon, clip, "ui", 0.1)).toBe(false);
		expect(weaponVisible(weapon, clip, "ui", 0.4)).toBe(true);
		expect(
			weaponVisible(weapon, "a_actor_chen_ui_weapon_to_overview.glb", "ui", 2),
		).toBe(false);
	});

	test("hides it for untracked menu clips and `hideWeapon` clips", () => {
		expect(
			weaponVisible(weapon, "a_actor_chen_ui_overview_loop.glb", "ui", 1),
		).toBe(false);
		expect(
			weaponVisible(
				weapon,
				"a_actor_chen_interact_camera_01.glb",
				"interact",
				1,
			),
		).toBe(false);
		expect(weaponVisible(weapon, null, null, 0)).toBe(false);
	});

	test("shows it for battle clips", () => {
		expect(
			weaponVisible(weapon, "a_actor_chen_battle_attack_01.glb", "battle", 0),
		).toBe(true);
	});
});

const DECO_RAW = {
	format: 1,
	id: "chr_0004_pelica",
	items: [
		{
			n: 1,
			prefab: "chr_0004_pelica_deco_1",
			file: "1.glb",
			mount: "IK_Weapon_R_001",
			kind: "deco",
			meshes: 1,
		},
		{
			n: 2,
			prefab: "chr_0004_pelica_deco_2",
			file: null,
			mount: "",
			kind: "deco",
			meshes: 1,
		},
		{ prefab: "no n" },
	],
	clips: {
		"a_actor_pelica_ui_overview_loop.glb": {
			"1": { visible: { Mesh_all: [[0, 1]] } },
			"2": { visible: { Mesh_all: [[0, 1]] } },
		},
		"a_actor_pelica_ui_relax_loop.glb": {
			"1": { visible: { Mesh_all: [[0, 0]] } },
		},
		"a_actor_pelica_ui_overview_to_weapon.glb": {
			"1": {
				visible: {
					"Mesh_all/lod0/part": [
						[0, 1],
						[1.5, 0],
					],
				},
			},
		},
	},
};

const deco = parseDecoIndex(DECO_RAW) as DecoIndex;

describe("parseDecoIndex / decoVisibility", () => {
	test("keeps items with a number and their mounts", () => {
		expect(deco.items).toHaveLength(2);
		expect(deco.items[0]).toMatchObject({
			n: 1,
			file: "1.glb",
			mount: "IK_Weapon_R_001",
		});
		expect(deco.items[1]).toMatchObject({ n: 2, file: null, mount: "" });
	});

	test("resolves per-node visibility at a time", () => {
		const [one, two] = deco.items;
		expect(
			decoVisibility(deco, one, "a_actor_pelica_ui_overview_loop.glb", 1),
		).toEqual({ Mesh_all: true });
		expect(
			decoVisibility(deco, one, "a_actor_pelica_ui_relax_loop.glb", 1),
		).toEqual({ Mesh_all: false });
		expect(
			decoVisibility(deco, one, "a_actor_pelica_ui_overview_to_weapon.glb", 2),
		).toEqual({ "Mesh_all/lod0/part": false });
		// Listed clip, item not mentioned: hidden.
		expect(
			decoVisibility(deco, two, "a_actor_pelica_ui_relax_loop.glb", 1),
		).toEqual({});
		// Unlisted clip: hidden.
		expect(
			decoVisibility(deco, one, "a_actor_pelica_battle_loop.glb", 0),
		).toEqual({});
		expect(decoVisibility(deco, one, null, 0)).toEqual({});
	});

	test("nodeName takes the last path segment", () => {
		expect(nodeName("Mesh_all/lod0/part")).toBe("part");
		expect(nodeName("Mesh_all")).toBe("Mesh_all");
	});
});
