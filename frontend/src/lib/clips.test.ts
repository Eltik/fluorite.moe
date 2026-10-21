import { describe, expect, test } from "bun:test";
import {
	type ClipEntry,
	clipDurationLabel,
	clipKey,
	clipLabel,
	clipStem,
	defaultClip,
	featuredGroups,
	filterClips,
	groupBySource,
	holdAtRest,
	parseClipIndex,
	sourceFromName,
	travelFromRest,
} from "@/lib/clips";

function entry(
	name: string,
	source: ClipEntry["source"],
	extra: Partial<ClipEntry> = {},
): ClipEntry {
	return {
		file: `a_actor_chen_${name.replace(/\s+/g, "_")}.glb`,
		name,
		clip: `A_actor_chen_${name.replace(/\s+/g, "_")}`,
		source,
		duration: 2.5,
		loop: false,
		...extra,
	};
}

const CHEN: ClipEntry[] = [
	entry("battle attack 02", "battle", { duration: 4.23 }),
	entry("battle attack 01", "battle", {
		duration: 3.6667,
		rootMotion: true,
		rootNode: "chr_0005_chen_postmodel",
		rootStart: [0, 0, 0.4564],
		rootEnd: [0, 0, 0.9127],
	}),
	entry("battle loop", "battle", { duration: 2, loop: true }),
	entry("battle skill ult", "battle", { duration: 10 }),
	entry("battle normal skill", "battle", { duration: 4.55 }),
	entry("battle hit heavy", "battle"),
	entry("idle to battle", "battle"),
	entry("dialog single idle knock", "dialog"),
	entry("dialog single bow", "dialog"),
	entry("dialog state idle loop", "dialog", { loop: true }),
	entry("relax loop", "3c", { loop: true }),
	entry("sprint dash l", "3c"),
	entry("idle to relax", "3c"),
	entry("hair roll", "other", { duration: 0.1667 }),
	entry("gacha", "ui"),
	entry("ui overview start", "ui", { duration: 5.25 }),
	entry("ui overview loop", "ui", { loop: true }),
	entry("ui relax sp 02", "ui", { duration: 10 }),
	entry("ui relax sp 01", "ui", { duration: 6.83 }),
	entry("ui relax react", "ui", { duration: 9.03 }),
	entry("uiteam idle 03", "ui", { loop: true }),
	entry("uiteam idle 01", "ui", { loop: true }),
	entry("uiteam idle 02", "ui", { loop: true }),
	entry("interact receive gift", "interact"),
	entry("interact sit loop", "interact", { loop: true }),
];

describe("clipStem", () => {
	test("strips the .glb extension only", () => {
		expect(clipStem("a_actor_chen_battle_attack_01.glb")).toBe(
			"a_actor_chen_battle_attack_01",
		);
		expect(clipStem("A_actor_chen_x.GLB")).toBe("A_actor_chen_x");
		expect(clipStem("no_extension")).toBe("no_extension");
	});
});

describe("parseClipIndex", () => {
	test("keeps well-formed entries and the root-motion fields", () => {
		const parsed = parseClipIndex([
			{
				file: "a_actor_chen_battle_attack_01.glb",
				name: "battle attack 01",
				clip: "A_actor_chen_battle_attack_01",
				source: "battle",
				duration: 3.6667,
				samples: 221,
				fps: 60,
				loop: false,
				humanoid: true,
				tracks: 371,
				matched: 371,
				size: 537716,
				rootMotion: true,
				rootNode: "chr_0005_chen_postmodel",
				rootStart: [0, 0, 0.4564],
				rootEnd: [0, 0, 0.9127],
			},
		]);
		expect(parsed).toHaveLength(1);
		expect(parsed[0]).toMatchObject({
			file: "a_actor_chen_battle_attack_01.glb",
			source: "battle",
			duration: 3.6667,
			loop: false,
			rootMotion: true,
			rootNode: "chr_0005_chen_postmodel",
			rootStart: [0, 0, 0.4564],
		});
	});

	test("fills defaults and drops junk", () => {
		const parsed = parseClipIndex([
			{ file: "a_actor_x_ui_idle.glb" },
			{ name: "no file" },
			null,
			"string",
			{ file: "a_actor_x_ui_idle.glb", name: "duplicate" },
			{ file: "b.glb", source: "bogus", duration: -1, loop: "yes" },
		]);
		expect(parsed.map((e) => e.file)).toEqual([
			"a_actor_x_ui_idle.glb",
			"b.glb",
		]);
		expect(parsed[0]).toMatchObject({
			name: "a actor x ui idle",
			clip: "a_actor_x_ui_idle",
			source: "ui",
			duration: 0,
			loop: false,
		});
		expect(parsed[1].source).toBe("other");
		expect(parsed[1].duration).toBe(0);
		expect(parsed[1].loop).toBe(false);
		expect(parsed[1].rootMotion).toBeUndefined();
	});

	test("keeps the root node of a clip that holds it off rest", () => {
		const [entry] = parseClipIndex([
			{
				file: "a_actor_jsspsi_uiteam_idle_01.glb",
				rootMotion: false,
				rootNode: "Root",
				rootStart: [-0.9333, 0, -1.1749],
				rootEnd: [-0.9333, 0, -1.1749],
			},
		]);
		expect(entry.rootMotion).toBeUndefined();
		expect(entry.rootNode).toBe("Root");
		expect(entry.rootStart).toEqual([-0.9333, 0, -1.1749]);
	});

	test("ignores non-array payloads", () => {
		expect(parseClipIndex(null)).toEqual([]);
		expect(parseClipIndex({ clips: [] })).toEqual([]);
	});
});

describe("sourceFromName", () => {
	test("buckets embedded clip names", () => {
		expect(sourceFromName("A_actor_chen_uiteam_idle_01")).toBe("ui");
		expect(sourceFromName("A_actor_chen_battle_attack_01")).toBe("battle");
		expect(sourceFromName("A_actor_chen_dialog_single_bow")).toBe("dialog");
		expect(sourceFromName("A_actor_chen_interact_sit_loop")).toBe("interact");
		expect(sourceFromName("A_actor_chen_walk_loop")).toBe("3c");
		expect(sourceFromName("A_actor_chen_hair_roll")).toBe("other");
	});
});

describe("labels", () => {
	test("sentence-cases the name and formats durations", () => {
		const e = entry("battle attack 01", "battle", { duration: 3.6667 });
		expect(clipLabel(e)).toBe("Battle attack 01");
		expect(clipDurationLabel(e)).toBe("3.7 s");
		expect(clipDurationLabel({ ...e, loop: true })).toBe("loop");
		expect(clipKey(entry("UI Team  idle 01", "ui"))).toBe("ui_team_idle_01");
	});
});

describe("filterClips", () => {
	test("matches every word against name, file and source", () => {
		expect(filterClips(CHEN, "")).toHaveLength(CHEN.length);
		expect(filterClips(CHEN, "attack").map((e) => e.name)).toEqual([
			"battle attack 02",
			"battle attack 01",
		]);
		expect(filterClips(CHEN, "ATTACK 01").map((e) => e.name)).toEqual([
			"battle attack 01",
		]);
		expect(filterClips(CHEN, "menu").every((e) => e.source === "ui")).toBe(
			true,
		);
		expect(filterClips(CHEN, "zzz")).toEqual([]);
	});
});

describe("groupBySource", () => {
	test("groups in fixed order and skips empty buckets", () => {
		const groups = groupBySource(CHEN);
		expect(groups.map((g) => g.id)).toEqual([
			"ui",
			"battle",
			"3c",
			"interact",
			"dialog",
			"other",
		]);
		expect(groups[0].label).toBe("Menu");
		expect(groups[0].items).toHaveLength(9);
		expect(
			groupBySource(CHEN.filter((e) => e.source === "ui")).map((g) => g.id),
		).toEqual(["ui"]);
	});
});

describe("featuredGroups", () => {
	const groups = featuredGroups(CHEN);
	const byId = Object.fromEntries(groups.map((g) => [g.id, g]));

	test("builds the menu group in the game's order", () => {
		expect(byId.menu.items.map((i) => i.label)).toEqual([
			"Overview start",
			"Overview",
			"Special 1",
			"Special 2",
			"Reaction",
			"Team Pose 1",
			"Team Pose 2",
			"Team Pose 3",
			"Receive Gift",
			"Gacha",
		]);
		expect(byId.menu.items[5].entry.file).toBe(
			"a_actor_chen_uiteam_idle_01.glb",
		);
	});

	test("builds the battle chain with the rest of the bucket appended", () => {
		expect(byId.battle.items.map((i) => i.label)).toEqual([
			"Draw weapon",
			"Battle stance",
			"Basic attack 1",
			"Basic attack 2",
			"Battle skill",
			"Ultimate",
			"Battle hit heavy",
		]);
	});

	test("movement keeps the 3c bucket, dialog keeps dialog idles", () => {
		expect(byId.movement.items.map((i) => i.label)).toEqual([
			"Relax loop",
			"Sprint dash l",
			"Idle to relax",
		]);
		expect(byId.dialog.items.map((i) => i.label)).toEqual([
			"Single idle knock",
			"State idle loop",
		]);
	});

	test("each clip appears once and helper clips are left out", () => {
		const files = groups.flatMap((g) => g.items.map((i) => i.entry.file));
		expect(new Set(files).size).toBe(files.length);
		expect(files).not.toContain("a_actor_chen_hair_roll.glb");
		expect(files).not.toContain("a_actor_chen_dialog_single_bow.glb");
	});

	test("enemy move names map to skills and attacks", () => {
		const enemy: ClipEntry[] = [
			{
				...entry("skill03 01 cast", "other"),
				file: "a_monster_x_skill03_01_cast.glb",
			},
			{ ...entry("skill01", "other"), file: "a_monster_x_skill01.glb" },
			{ ...entry("skill02 03", "other"), file: "a_monster_x_skill02_03.glb" },
			{ ...entry("head hit", "other"), file: "a_monster_x_head_hit.glb" },
			{
				...entry("jumpleft loop", "other", { loop: true }),
				file: "a_monster_x_jumpleft_loop.glb",
			},
		];
		const g = featuredGroups(enemy);
		expect(g.map((x) => x.id)).toEqual(["battle"]);
		expect(g[0].items.map((i) => i.label)).toEqual([
			"Skill 1",
			"Skill 2.3",
			"Skill 3.1 cast",
		]);
	});

	test("falls back to dialog loops when a model has no dialog idles", () => {
		const npc: ClipEntry[] = [
			entry("major andrew dialog state scarelook start", "dialog"),
			entry("major andrew dialog state scarelook loop", "dialog", {
				loop: true,
			}),
			entry("major andrew customized single hug", "other"),
		];
		const g = featuredGroups(npc);
		expect(g.map((x) => x.id)).toEqual(["dialog"]);
		expect(g[0].items.map((i) => i.label)).toEqual([
			"Major andrew dialog state scarelook loop",
		]);
	});

	test("omits empty groups", () => {
		expect(featuredGroups([])).toEqual([]);
		expect(featuredGroups([entry("hair roll", "other")])).toEqual([]);
	});
});

describe("defaultClip", () => {
	test("prefers the first team pose", () => {
		expect(defaultClip(CHEN)?.file).toBe("a_actor_chen_uiteam_idle_01.glb");
	});
	test("falls back through menu loops, idle loops and any loop", () => {
		const noTeam = CHEN.filter((e) => !e.name.startsWith("uiteam"));
		expect(defaultClip(noTeam)?.name).toBe("ui overview loop");
		const noUi = noTeam.filter((e) => e.source !== "ui");
		expect(defaultClip(noUi)?.name).toBe("dialog state idle loop");
		const noIdle = noUi.filter((e) => !/idle/.test(e.name));
		expect(defaultClip(noIdle)?.name).toBe("battle loop");
		expect(
			defaultClip([
				entry("customized state bewitched loop", "other", { loop: true }),
				entry("dialog state standsleep loop", "dialog", { loop: true }),
			])?.name,
		).toBe("dialog state standsleep loop");
		expect(
			defaultClip([entry("hair roll", "other"), entry("skill01", "other")])
				?.name,
		).toBe("skill01");
		expect(defaultClip([])).toBeUndefined();
	});
});

describe("root placement", () => {
	// Si's Team Pose 1: the generic root bone parks at the lineup slot.
	const parked = [-0.9333, 0, -1.1749, -0.9333, 0, -1.1749];
	// A clip that travels 0.46 m forward from 0.46 m and hops 0.2 m.
	const travel = [0, 0, 0.4564, 0.1, 0.2, 0.9127];

	test("in place holds the ground position at rest and keeps height", () => {
		expect(Array.from(holdAtRest(parked, [0, 0, 0]))).toEqual([
			0, 0, 0, 0, 0, 0,
		]);
		const held = Array.from(holdAtRest(travel, [0, 0, 0]));
		expect(held[1]).toBe(0);
		expect(held[4]).toBeCloseTo(0.2);
		expect([held[0], held[2], held[3], held[5]]).toEqual([0, 0, 0, 0]);
	});

	test("root motion travels by the clip's delta from rest", () => {
		const moved = Array.from(travelFromRest(travel, [0, 0, 0]));
		expect(moved[0]).toBeCloseTo(0);
		expect(moved[2]).toBeCloseTo(0);
		expect(moved[3]).toBeCloseTo(0.1);
		expect(moved[4]).toBeCloseTo(0.2);
		expect(moved[5]).toBeCloseTo(0.4563);
		// A parked root has no delta, so it stands at rest too.
		expect(Array.from(travelFromRest(parked, [0, 0, 0]))).toEqual([
			0, 0, 0, 0, 0, 0,
		]);
	});
});
