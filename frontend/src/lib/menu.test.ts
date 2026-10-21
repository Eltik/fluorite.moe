import { describe, expect, test } from "bun:test";
import {
	crossed,
	defaultEntry,
	type MenuMachine,
	MOMENT_FADE,
	momentAvailable,
	nextExit,
	parseMenu,
	planMoment,
	stateClip,
	upcomingClips,
} from "@/lib/menu";

/** A trimmed `menu.json` in the exporter's shape. */
const RAW = {
	format: 1,
	id: "chr_0036_jsspsi",
	default: "Overview.OverviewIdle",
	states: {
		"Overview.FromOveview": {
			clip: "a_actor_jsspsi_ui_overview_start_01.glb",
			length: 12.4167,
			loop: false,
			exit: {
				to: "Overview.OverviewIdle",
				at: 12.3998,
				fade: 0.0168,
				offset: 0,
			},
		},
		"Overview.OverviewIdle": {
			clip: "a_actor_jsspsi_ui_overview_loop_01.glb",
			length: 2.5,
			loop: true,
			exit: null,
		},
		"Weapon.FromOverview": {
			clip: "a_actor_jsspsi_ui_overview_to_weapon_01.glb",
			length: 3.8333,
			loop: false,
			exit: { to: "Weapon.WeaponIdle", at: 3.8196, fade: 0.013, offset: 0 },
		},
		"Weapon.WeaponIdle": {
			clip: "a_actor_jsspsi_ui_overview_weapon_loop_01.glb",
			length: 2.5,
			loop: true,
			exit: null,
		},
		"Formation.Slot0": {
			clip: "a_actor_jsspsi_ui_overview_loop_01.glb",
			length: 2.5,
			loop: true,
			exit: { to: "Formation.Slot1", at: 2.3438, fade: 0.25, offset: 0 },
		},
		"Formation.Slot1": {
			clip: "a_actor_jsspsi_ui_overview_loop_01.glb",
			length: 2.5,
			loop: true,
			exit: null,
		},
		"Relax.RelaxIdle": {
			clip: "a_actor_jsspsi_ui_relax_loop.glb",
			length: 3,
			loop: true,
			exit: null,
			on: [
				{
					when: { EnableSwitch: true, FromIndex: 6, ToIndex: 20 },
					to: "Relax.SP1",
					at: 2.7353,
					fade: 0.2226,
					offset: 0,
				},
			],
		},
		"Relax.SP1": {
			clip: "a_actor_jsspsi_ui_relax_sp_01.glb",
			length: 11.35,
			loop: false,
			exit: { to: "Relax.RelaxIdle", at: 10.7825, fade: 1.3556, offset: 0 },
		},
		"Gift.Receive": {
			clip: null,
			length: 4,
			loop: false,
			exit: { to: "Overview.OverviewIdle", at: 3.9, fade: 0.1, offset: 0 },
			missingClip: true,
		},
		"Broken.ToMissing": {
			clip: "a_actor_jsspsi_ui_broken.glb",
			length: 1,
			loop: false,
			exit: { to: "Gift.Receive", at: 0.9, fade: 0.1, offset: 0.5 },
		},
	},
	any: [{ when: { SkipIn: true }, to: "Overview.OverviewIdle", fade: 0 }],
	params: { WeaponHide: "float" },
	moments: [
		{
			id: "overview",
			label: "Overview",
			send: { kind: "page", index: 0 },
			state: "Overview.FromOveview",
			idle: "Overview.OverviewIdle",
		},
		{
			id: "weapon",
			label: "Weapon",
			send: { kind: "page", index: 1 },
			state: "Weapon.FromOverview",
			idle: "Weapon.WeaponIdle",
		},
		{
			id: "special1",
			label: "Special 1",
			send: { kind: "relax", index: 20 },
			state: "Relax.SP1",
			idle: "Relax.RelaxIdle",
			from: "Relax.RelaxIdle",
		},
		{
			id: "gift",
			label: "Receive Gift",
			send: { kind: "page", index: 9 },
			state: "Gift.Receive",
			idle: "Overview.OverviewIdle",
		},
		{ id: "overview", label: "Duplicate", state: "Overview.OverviewIdle" },
		{ label: "no id", state: "Overview.OverviewIdle" },
	],
};

const machine = parseMenu(RAW) as MenuMachine;

describe("parseMenu", () => {
	test("keeps states, transitions and moments", () => {
		expect(machine).not.toBeNull();
		expect(machine.default).toBe("Overview.OverviewIdle");
		expect(Object.keys(machine.states)).toHaveLength(10);
		expect(machine.states["Weapon.FromOverview"]).toMatchObject({
			clip: "a_actor_jsspsi_ui_overview_to_weapon_01.glb",
			loop: false,
			exit: { to: "Weapon.WeaponIdle", at: 3.8196, fade: 0.013, offset: 0 },
		});
		expect(machine.states["Relax.RelaxIdle"].on).toHaveLength(1);
		expect(machine.states["Gift.Receive"]).toMatchObject({
			clip: null,
			missingClip: true,
		});
	});

	test("drops duplicate and nameless moments, keeps `from`", () => {
		expect(machine.moments.map((m) => m.id)).toEqual([
			"overview",
			"weapon",
			"special1",
			"gift",
		]);
		expect(machine.moments[2].from).toBe("Relax.RelaxIdle");
		expect(machine.moments[0].from).toBeUndefined();
	});

	test("rejects payloads without states", () => {
		expect(parseMenu(null)).toBeNull();
		expect(parseMenu({ format: 1 })).toBeNull();
		expect(parseMenu({ states: {} })).toBeNull();
	});

	test("accepts `fadeOfSource` and fills a default", () => {
		const m = parseMenu({
			states: {
				A: { clip: "a.glb", loop: false, exit: { to: "B", fadeOfSource: 0.5 } },
				B: { clip: "b.glb", loop: true },
			},
		}) as MenuMachine;
		expect(m.states.A.exit).toEqual({
			to: "B",
			at: null,
			fade: 0.5,
			offset: 0,
		});
		expect(m.default).toBe("B");
	});
});

describe("momentAvailable / defaultEntry", () => {
	test("moments whose state has no exported clip are unavailable", () => {
		const [overview, , , gift] = machine.moments;
		expect(momentAvailable(machine, overview)).toBe(true);
		expect(momentAvailable(machine, gift)).toBe(false);
	});

	test("the model opens on the moment that settles into the default loop", () => {
		expect(defaultEntry(machine)).toEqual({
			moment: machine.moments[0],
			state: "Overview.FromOveview",
		});
	});

	test("falls back to the default loop without moments", () => {
		const m = parseMenu({
			default: "Idle",
			states: { Idle: { clip: "idle.glb", loop: true } },
		}) as MenuMachine;
		expect(defaultEntry(m)).toEqual({ moment: null, state: "Idle" });
	});
});

describe("planMoment", () => {
	test("plays the moment's state behind the default crossfade", () => {
		expect(planMoment(machine, "weapon", "Overview.OverviewIdle")).toEqual({
			state: "Weapon.FromOverview",
			fade: MOMENT_FADE,
			offset: 0,
			at: null,
		});
	});

	test("honours the timed `on` rule when leaving the moment's `from` state", () => {
		expect(planMoment(machine, "special1", "Relax.RelaxIdle")).toEqual({
			state: "Relax.SP1",
			fade: 0.2226,
			offset: 0,
			at: 2.7353,
		});
		// From anywhere else it just starts.
		expect(planMoment(machine, "special1", "Weapon.WeaponIdle")?.at).toBeNull();
	});

	test("refuses unknown and unexported moments", () => {
		expect(planMoment(machine, "nope", null)).toBeNull();
		expect(planMoment(machine, "gift", null)).toBeNull();
	});
});

describe("crossed", () => {
	test("fires once when the time passes `at`", () => {
		expect(crossed(3.8, 3.7, 3.85, false)).toBe(true);
		expect(crossed(3.8, 3.85, 3.9, false)).toBe(false);
		expect(crossed(3.8, 3.0, 3.5, false)).toBe(false);
	});

	test("a wrapped loop counts the keys it skipped", () => {
		expect(crossed(2.34, 2.3, 0.1, true)).toBe(true);
		expect(crossed(0.05, 2.3, 0.1, true)).toBe(true);
		expect(crossed(1.0, 2.3, 0.1, true)).toBe(false);
	});

	test("a fresh state fires when it starts past `at`", () => {
		expect(crossed(0.5, -1, 0.6, false)).toBe(true);
		expect(crossed(0.5, -1, 0.1, false)).toBe(false);
	});
});

describe("nextExit", () => {
	test("follows the overview start into its loop at the exit time", () => {
		expect(
			nextExit(machine, "Overview.FromOveview", 12.3, 12.41),
		).toMatchObject({ to: "Overview.OverviewIdle", fade: 0.0168 });
		expect(nextExit(machine, "Overview.FromOveview", 5, 5.1)).toBeNull();
	});

	test("loops without an exit never leave", () => {
		expect(nextExit(machine, "Overview.OverviewIdle", 2.4, 0.1)).toBeNull();
	});

	test("looping states with an exit fire across the wrap", () => {
		expect(nextExit(machine, "Formation.Slot0", 2.3, 0.05)).toMatchObject({
			to: "Formation.Slot1",
		});
	});

	test("skips exits into states without a clip", () => {
		expect(nextExit(machine, "Broken.ToMissing", 0.8, 0.95)).toBeNull();
	});
});

describe("stateClip / upcomingClips", () => {
	test("reports the exported clip and what may follow", () => {
		expect(stateClip(machine, "Gift.Receive")).toBeNull();
		expect(stateClip(machine, "Weapon.WeaponIdle")).toBe(
			"a_actor_jsspsi_ui_overview_weapon_loop_01.glb",
		);
		expect(upcomingClips(machine, "Weapon.FromOverview")).toEqual([
			"a_actor_jsspsi_ui_overview_weapon_loop_01.glb",
		]);
		expect(upcomingClips(machine, "Relax.RelaxIdle")).toEqual([
			"a_actor_jsspsi_ui_relax_sp_01.glb",
		]);
		expect(upcomingClips(machine, "Broken.ToMissing")).toEqual([]);
	});
});
