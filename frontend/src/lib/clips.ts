/**
 * Animation clip index helpers for the Studio. Pure functions over the
 * `clips/<kind>/<id>/index.json` entries the exporter writes next to the
 * per-clip GLBs: validation, search, grouping and the "Featured" curation
 * built from clip-name patterns (until the in-game state machine ships).
 * Kept free of three.js so the catalog and tests can use it.
 */

export const CLIP_SOURCES = [
	"ui",
	"battle",
	"3c",
	"interact",
	"dialog",
	"other",
] as const;
export type ClipSource = (typeof CLIP_SOURCES)[number];

export const CLIP_SOURCE_LABELS: Record<ClipSource, string> = {
	ui: "Menu",
	battle: "Battle",
	"3c": "Movement",
	interact: "Interact",
	dialog: "Dialog",
	other: "Other",
};

export type Vec3 = [number, number, number];

/** One entry of a clip index (see the exporter's `index.json`). */
export interface ClipEntry {
	/** GLB file name inside the clip folder, e.g. `a_actor_chen_battle_attack_01.glb`. */
	file: string;
	/** Prettified name with the actor prefix stripped (`battle attack 01`). */
	name: string;
	/** The clip's name inside the GLB. */
	clip: string;
	source: ClipSource;
	duration: number;
	loop: boolean;
	samples?: number;
	fps?: number;
	humanoid?: boolean;
	tracks?: number;
	matched?: number;
	size?: number;
	/** The clip moves `rootNode` from `rootStart` to `rootEnd`. */
	rootMotion?: boolean;
	/**
	 * The node that carries the clip's travel: the Animator's node for
	 * humanoid clips, the Avatar's root motion bone (`Root`) for generic ones.
	 * Playing in place holds it at its rest position.
	 */
	rootNode?: string;
	rootStart?: Vec3;
	rootEnd?: Vec3;
}

/** `a_actor_chen_battle_attack_01.glb` → `a_actor_chen_battle_attack_01`. */
export function clipStem(file: string): string {
	return file.replace(/\.glb$/i, "");
}

export function isClipSource(value: unknown): value is ClipSource {
	return (
		typeof value === "string" &&
		(CLIP_SOURCES as readonly string[]).includes(value)
	);
}

function vec3(value: unknown): Vec3 | undefined {
	return Array.isArray(value) &&
		value.length === 3 &&
		value.every((n) => typeof n === "number" && Number.isFinite(n))
		? (value as Vec3)
		: undefined;
}

function num(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value)
		? value
		: undefined;
}

/**
 * Guesses the source bucket from a clip name, for GLBs that embed their
 * clips without an index (dev files, older exports).
 */
export function sourceFromName(name: string): ClipSource {
	const key = name.toLowerCase();
	if (/(^|[_\s])(ui|uiteam|gacha)([_\s]|$)/.test(key)) return "ui";
	if (/(^|[_\s])battle([_\s]|$)/.test(key)) return "battle";
	if (/(^|[_\s])dialog([_\s]|$)/.test(key)) return "dialog";
	if (/(^|[_\s])interact([_\s]|$)/.test(key)) return "interact";
	if (/(^|[_\s])(idle|walk|run|sprint|jump|relax|stand)([_\s]|$)/.test(key)) {
		return "3c";
	}
	return "other";
}

/**
 * Validates an index payload. Entries without a file name are dropped;
 * missing fields get safe defaults so a slightly older index still browses.
 */
export function parseClipIndex(json: unknown): ClipEntry[] {
	if (!Array.isArray(json)) return [];
	const out: ClipEntry[] = [];
	const seen = new Set<string>();
	for (const raw of json) {
		if (!raw || typeof raw !== "object") continue;
		const r = raw as Record<string, unknown>;
		if (typeof r.file !== "string" || !r.file || seen.has(r.file)) continue;
		seen.add(r.file);
		const stem = clipStem(r.file);
		const name =
			typeof r.name === "string" && r.name.trim()
				? r.name.trim()
				: stem.replace(/_/g, " ");
		const entry: ClipEntry = {
			file: r.file,
			name,
			clip: typeof r.clip === "string" && r.clip ? r.clip : stem,
			source: isClipSource(r.source) ? r.source : sourceFromName(name),
			duration: Math.max(0, num(r.duration) ?? 0),
			loop: r.loop === true,
		};
		const samples = num(r.samples);
		if (samples !== undefined) entry.samples = samples;
		const fps = num(r.fps);
		if (fps !== undefined) entry.fps = fps;
		if (typeof r.humanoid === "boolean") entry.humanoid = r.humanoid;
		const tracks = num(r.tracks);
		if (tracks !== undefined) entry.tracks = tracks;
		const matched = num(r.matched);
		if (matched !== undefined) entry.matched = matched;
		const size = num(r.size);
		if (size !== undefined) entry.size = size;
		// The root node is kept for clips that hold it away from its rest
		// position without moving it (team poses stand at their lineup slot),
		// so playing in place can pin those too.
		if (typeof r.rootNode === "string" && r.rootNode) {
			if (r.rootMotion === true) entry.rootMotion = true;
			entry.rootNode = r.rootNode;
			const start = vec3(r.rootStart);
			if (start) entry.rootStart = start;
			const end = vec3(r.rootEnd);
			if (end) entry.rootEnd = end;
		}
		out.push(entry);
	}
	return out;
}

// ------------------------------------------------------------- root placement

/**
 * Root positions (flat `x, y, z` keys) held at the rest ground position: x
 * and z become the rest's, y keeps the clip's height. How a clip plays in
 * place, including one that parks its root off rest without moving it.
 */
export function holdAtRest(
	values: ArrayLike<number>,
	rest: Vec3,
): Float32Array {
	const out = Float32Array.from(values);
	for (let i = 0; i + 2 < out.length; i += 3) {
		out[i] = rest[0];
		out[i + 2] = rest[2];
	}
	return out;
}

/**
 * Root positions moved so the clip's ground travel starts at the rest
 * position (the game applies root motion as a delta from where the model
 * stands); y keeps the clip's height.
 */
export function travelFromRest(
	values: ArrayLike<number>,
	rest: Vec3,
): Float32Array {
	const out = Float32Array.from(values);
	if (out.length < 3) return out;
	const dx = rest[0] - out[0];
	const dz = rest[2] - out[2];
	for (let i = 0; i + 2 < out.length; i += 3) {
		out[i] += dx;
		out[i + 2] += dz;
	}
	return out;
}

// ------------------------------------------------------------- labels

/** `battle attack 01` → `Battle attack 01`. */
export function sentenceCase(text: string): string {
	const t = text.trim();
	return t ? t.charAt(0).toUpperCase() + t.slice(1) : t;
}

/** The row label for a clip: its prettified name in sentence case. */
export function clipLabel(entry: ClipEntry): string {
	return sentenceCase(entry.name);
}

/** `loop` for looping clips, else the duration in seconds (`3.7 s`). */
export function clipDurationLabel(entry: ClipEntry): string {
	return entry.loop ? "loop" : `${entry.duration.toFixed(1)} s`;
}

/** The underscore key used for pattern matching: `uiteam idle 01` → `uiteam_idle_01`. */
export function clipKey(entry: ClipEntry): string {
	return entry.name.toLowerCase().trim().replace(/\s+/g, "_");
}

// ------------------------------------------------------------- search

/** Case-insensitive match: every word of the query must appear in `text`. */
export function matchesQuery(text: string, query: string): boolean {
	const words = query.toLowerCase().split(/\s+/).filter(Boolean);
	if (!words.length) return true;
	const hay = text.toLowerCase();
	return words.every((w) => hay.includes(w));
}

/** Entries whose name, file or source matches every word of the query. */
export function filterClips(
	entries: readonly ClipEntry[],
	query: string,
): ClipEntry[] {
	if (!query.trim()) return [...entries];
	return entries.filter((e) =>
		matchesQuery(
			`${e.name} ${e.file} ${e.source} ${CLIP_SOURCE_LABELS[e.source]}`,
			query,
		),
	);
}

export interface ClipGroup<T = ClipEntry> {
	id: string;
	label: string;
	items: T[];
}

/** Groups entries by source, in `CLIP_SOURCES` order, skipping empty buckets. */
export function groupBySource(
	entries: readonly ClipEntry[],
): ClipGroup<ClipEntry>[] {
	const buckets = new Map<ClipSource, ClipEntry[]>();
	for (const e of entries) {
		const list = buckets.get(e.source);
		if (list) list.push(e);
		else buckets.set(e.source, [e]);
	}
	return CLIP_SOURCES.filter((s) => buckets.has(s)).map((s) => ({
		id: s,
		label: CLIP_SOURCE_LABELS[s],
		items: buckets.get(s) as ClipEntry[],
	}));
}

// ------------------------------------------------------------- featured

export interface FeaturedItem {
	entry: ClipEntry;
	label: string;
}

export type FeaturedGroupId = "menu" | "battle" | "movement" | "dialog";

export type FeaturedGroup = ClipGroup<FeaturedItem> & { id: FeaturedGroupId };

interface Rule {
	test: RegExp;
	label: string | ((m: RegExpExecArray) => string);
	/** Restrict the rule to these sources (default: any). */
	sources?: readonly ClipSource[];
}

const n = (s: string | undefined): string => String(Number(s ?? "1"));

/** Character-screen moments, in the order the game's menu lists them. */
const MENU_RULES: Rule[] = [
	{ test: /^ui_overview_start$/, label: "Overview start" },
	{ test: /^ui_overview_loop$/, label: "Overview" },
	{ test: /^ui_overview_weapon_loop$/, label: "Weapon" },
	{ test: /^ui_overview_equip_loop$/, label: "Gear" },
	{ test: /^ui_overview_skill_loop$/, label: "Ability" },
	{ test: /^ui_upgrade_loop$/, label: "Upgrade" },
	{ test: /^ui_relax_loop$/, label: "Preview" },
	{ test: /^ui_relax_sp_(\d+)$/, label: (m) => `Special ${n(m[1])}` },
	{ test: /^ui_relax_react$/, label: "Reaction" },
	{ test: /^uiteam_idle_(\d+)$/, label: (m) => `Team Pose ${n(m[1])}` },
	{ test: /^interact_receive_gift$/, label: "Receive Gift" },
	{ test: /^gacha$/, label: "Gacha" },
];

const BATTLE_RULES: Rule[] = [
	{ test: /^idle_to_battle$/, label: "Draw weapon" },
	{ test: /^battle_loop$/, label: "Battle stance" },
	{ test: /^battle_attack_(\d+)$/, label: (m) => `Basic attack ${n(m[1])}` },
	{ test: /^battle_power_attack$/, label: "Finisher" },
	{ test: /^battle_dash_attack$/, label: "Dash attack" },
	{ test: /^battle_air_atk_start$/, label: "Dive attack start" },
	{ test: /^battle_air_atk_loop$/, label: "Dive attack loop" },
	{ test: /^battle_air_atk_end$/, label: "Dive attack end" },
	{ test: /^battle_normal_skill$/, label: "Battle skill" },
	{ test: /^battle_combo_skill$/, label: "Combo skill" },
	{ test: /^battle_(skill_)?ult(imate)?$/, label: "Ultimate" },
	{ test: /^battle_dodge_(\d+)$/, label: (m) => `Dodge ${n(m[1])}` },
	{ test: /^battle_double_dash$/, label: "Double dash" },
	{ test: /^battle_to_idle$/, label: "Sheathe weapon" },
	{ test: /^battle_dead$/, label: "Defeat" },
	// Enemies and NPCs name their moves without a `battle_` prefix.
	{
		test: /^(?:taunt|entrance|born|appear|spawn)(?:_\d+)?$/,
		label: "Entrance",
		sources: ["other", "battle"],
	},
	{
		test: /^(?:idle|battle_idle|stand)(?:_loop)?$/,
		label: "Idle",
		sources: ["other", "battle"],
	},
	{
		test: /^attack(?:_?(\d+))?$/,
		label: (m) => (m[1] ? `Attack ${n(m[1])}` : "Attack"),
		sources: ["other", "battle"],
	},
	{
		test: /^skill_?(\d+)(?:_(\d+))?(?:_([a-z]+))?$/,
		label: (m) =>
			[m[2] ? `Skill ${n(m[1])}.${n(m[2])}` : `Skill ${n(m[1])}`, m[3] ?? ""]
				.join(" ")
				.trim(),
		sources: ["other", "battle"],
	},
	{
		test: /^(?:dead|death|die|defeat)(?:_\d+)?$/,
		label: "Defeat",
		sources: ["other", "battle"],
	},
];

/** Locomotion first, then the rest of the `3c` bucket. */
const MOVEMENT_RULES: Rule[] = [
	{ test: /^idle(?:_loop)?$/, label: "Idle", sources: ["3c"] },
	{ test: /^walk(?:_loop)?$/, label: "Walk", sources: ["3c"] },
	{ test: /^run(?:_loop)?$/, label: "Run", sources: ["3c"] },
	{ test: /^sprint(?:_loop)?$/, label: "Sprint", sources: ["3c"] },
	{ test: /^jump(?:_start|_loop|_end)?$/, label: "Jump", sources: ["3c"] },
];

function applyRules(
	entries: readonly ClipEntry[],
	rules: readonly Rule[],
	used: Set<string>,
): FeaturedItem[] {
	const items: FeaturedItem[] = [];
	for (const rule of rules) {
		// Matches of one rule sort by their captured numbers (attack 1, 2, …),
		// since the index is ordered by file name, not by sequence.
		const matches: { entry: ClipEntry; m: RegExpExecArray; order: number[] }[] =
			[];
		for (const entry of entries) {
			if (used.has(entry.file)) continue;
			if (rule.sources && !rule.sources.includes(entry.source)) continue;
			const m = rule.test.exec(clipKey(entry));
			if (!m) continue;
			used.add(entry.file);
			matches.push({
				entry,
				m,
				order: m.slice(1).map((g) => (g === undefined ? -1 : Number(g))),
			});
		}
		matches.sort((a, b) => {
			for (let i = 0; i < Math.max(a.order.length, b.order.length); i++) {
				const d = (a.order[i] ?? -1) - (b.order[i] ?? -1);
				if (d) return d;
			}
			return 0;
		});
		for (const { entry, m } of matches) {
			items.push({
				entry,
				label: typeof rule.label === "string" ? rule.label : rule.label(m),
			});
		}
	}
	return items;
}

function rest(
	entries: readonly ClipEntry[],
	used: Set<string>,
	keep: (entry: ClipEntry) => boolean,
	label: (entry: ClipEntry) => string = clipLabel,
): FeaturedItem[] {
	const items: FeaturedItem[] = [];
	for (const entry of entries) {
		if (used.has(entry.file) || !keep(entry)) continue;
		used.add(entry.file);
		items.push({ entry, label: label(entry) });
	}
	return items;
}

const DIALOG_IDLE = /(^|_)idle(_|$)/;
/** Tiny pose-correction clips (hair, cloth, look-at), never worth featuring. */
const HELPER_CLIP = /^(?:hair|cloth|lookat|tail|onlycloth|t_pose)/;

/**
 * Curated groups built from clip-name patterns. Each clip appears at most
 * once; groups with nothing to show are omitted.
 */
export function featuredGroups(entries: readonly ClipEntry[]): FeaturedGroup[] {
	const used = new Set<string>();
	const menu = applyRules(entries, MENU_RULES, used);
	const battle = [
		...applyRules(entries, BATTLE_RULES, used),
		// Any remaining battle moves (hits, blown, walk stops…), then enemy
		// moves that live in `other` and look like attacks or skills.
		...rest(entries, used, (e) => e.source === "battle"),
		...rest(
			entries,
			used,
			(e) =>
				e.source === "other" &&
				/^(?:skill|attack|hit|dead|death|taunt|born|cast)/.test(clipKey(e)),
		),
	];
	const movement = [
		...applyRules(entries, MOVEMENT_RULES, used),
		...rest(
			entries,
			used,
			(e) => e.source === "3c" && !HELPER_CLIP.test(clipKey(e)),
		),
	];
	const dialogLabel = (e: ClipEntry) =>
		sentenceCase(e.name.replace(/^dialog\s+/i, ""));
	let dialog = rest(
		entries,
		used,
		(e) => e.source === "dialog" && DIALOG_IDLE.test(clipKey(e)),
		dialogLabel,
	);
	// NPCs name their dialog states after moods rather than "idle"; their
	// looping states are the equivalent.
	if (!dialog.length) {
		dialog = rest(
			entries,
			used,
			(e) => e.source === "dialog" && e.loop,
			dialogLabel,
		);
	}
	const groups: FeaturedGroup[] = [
		{ id: "menu", label: "Menu", items: menu },
		{ id: "battle", label: "Battle", items: battle },
		{ id: "movement", label: "Movement", items: movement },
		{ id: "dialog", label: "Dialog", items: dialog },
	];
	return groups.filter((g) => g.items.length > 0);
}

/**
 * What to play when a model opens: the first team pose, else a menu loop,
 * else any idle loop, else any loop, else the first clip.
 */
export function defaultClip(
	entries: readonly ClipEntry[],
): ClipEntry | undefined {
	if (!entries.length) return undefined;
	const byKey = (re: RegExp, extra?: (e: ClipEntry) => boolean) =>
		entries.find((e) => re.test(clipKey(e)) && (extra ? extra(e) : true));
	return (
		byKey(/^uiteam_idle_01$/) ??
		byKey(/^uiteam_idle_\d+$/) ??
		byKey(/^ui_overview_loop$/) ??
		entries.find((e) => e.source === "ui" && e.loop) ??
		byKey(/(^|_)(idle|stand[a-z]*)(_|$)/, (e) => e.loop) ??
		entries.find((e) => e.loop && !HELPER_CLIP.test(clipKey(e))) ??
		entries.find((e) => !HELPER_CLIP.test(clipKey(e))) ??
		entries[0]
	);
}
