/**
 * Display helpers for the story index: turn the API's raw group / dialog ids
 * (`a1m2`, `dlg_a1m2_3`, `map01/lv002`, `dlg_npc_0005_chen_spaceshipgift`)
 * into chapter sections, card titles and part labels, and remember the last
 * dialog a reader opened.
 *
 * Family names (main story, operator stories, ...) are inferred from the id
 * prefixes and the mission titles the game gives them; they are not game text.
 */
import type { StoryCategoryId, StoryGroup } from "@/lib/catalog";

interface OperatorRef {
	id: string;
	name: string;
	icon?: string | null;
}

export interface StoryCard {
	/** Stable key; also the search term that finds this card. */
	key: string;
	title: string;
	subtitle?: string;
	icon?: string;
	dialogs: string[];
	/** Dialog id → short chip label. */
	labels: Map<string, string>;
}

export interface StorySection {
	key: string;
	title: string;
	/** Family label shown above the title (e.g. "Main story"). */
	family?: string;
	icon?: string;
	cards: StoryCard[];
}

/** Operator lookup by the 4-digit character number (`0005` → Chen Qianyu). */
export type OperatorIndex = Map<number, OperatorRef>;

export function operatorIndex(items: OperatorRef[] | undefined): OperatorIndex {
	const map: OperatorIndex = new Map();
	for (const item of items ?? []) {
		const m = /^chr_(\d{4})_/.exec(item.id);
		// The Endministrator has two entries; keep the first.
		if (m && !map.has(Number(m[1]))) map.set(Number(m[1]), item);
	}
	return map;
}

/** Dialog suffixes that are UI / bookkeeping hooks, not story. */
const SYSTEM_SUFFIX = /^(OpenUI|NewSeries)$/i;

export function isSystemDialog(dialogId: string): boolean {
	const suffix = dialogId.split("_").at(-1) ?? "";
	return SYSTEM_SUFFIX.test(suffix);
}

function titleCase(words: string): string {
	return words
		.replace(/([a-z])([A-Z])/g, "$1 $2")
		.split(/[_\s]+/)
		.filter(Boolean)
		.map((w) => w[0]?.toUpperCase() + w.slice(1).toLowerCase())
		.join(" ");
}

/** `3` → `Part 3`, `3d5` → `Part 3.5`, `firstvisit` → `First visit`. */
export function partLabel(suffix: string): string {
	if (/^\d+$/.test(suffix)) return `Part ${Number(suffix)}`;
	const sub = /^(\d+)d(\d+)$/.exec(suffix);
	if (sub) return `Part ${Number(sub[1])}.${Number(sub[2])}`;
	const known: Record<string, string> = {
		firstvisit: "First visit",
		sealoff: "Seal-off",
		spaceshipgift: "Gift",
		spaceshippresent: "Present",
		openui: "UI hook",
		newseries: "New series",
	};
	const k = known[suffix.toLowerCase()];
	if (k) return k;
	const hub = /^hub(\d+)$/i.exec(suffix);
	if (hub) return `Hub ${hub[1]}`;
	const sentence = titleCase(suffix);
	return sentence.charAt(0) + sentence.slice(1).toLowerCase();
}

function dialogSuffix(dialogId: string, groupId: string): string {
	const prefix = `dlg_${groupId.replace(/\//g, "_")}_`;
	if (groupId && dialogId.startsWith(prefix))
		return dialogId.slice(prefix.length);
	return dialogId.replace(/^dlg_/, "");
}

/** Natural order: `1, 2, 3d5, 10, firstvisit`. */
function naturalCompare(a: string, b: string): number {
	return a.localeCompare(b, undefined, { numeric: true });
}

// ---------------------------------------------------------------- missions

interface Family {
	order: number;
	name: string;
	section: (n: number, ops: OperatorIndex) => { title: string; icon?: string };
}

const FAMILIES: Record<string, Family> = {
	e: {
		order: 0,
		name: "Main story",
		section: (n) => ({ title: n === 0 ? "Prologue" : `Chapter ${n}` }),
	},
	c: {
		order: 1,
		name: "Operator stories",
		section: (n, ops) => {
			const op = ops.get(n);
			return op
				? { title: op.name, icon: op.icon ?? undefined }
				: { title: `Operator story ${n}` };
		},
	},
	a: { order: 2, name: "Side arcs", section: (n) => ({ title: `Arc ${n}` }) },
	sm: {
		order: 3,
		name: "Side stories",
		section: (n) => ({ title: `Series ${n}` }),
	},
	gm: {
		order: 4,
		name: "Regional missions",
		section: (n) => ({ title: `Region ${n}` }),
	},
	f: {
		order: 5,
		name: "Facility missions",
		section: (n) => ({ title: `Facilities ${n}` }),
	},
	db: {
		order: 6,
		name: "Etchspace",
		section: (n) => ({ title: `Etchspace ${n}` }),
	},
	m: {
		order: 7,
		name: "Daily missions",
		section: (n) => ({ title: `Set ${n}` }),
	},
};

function missionSections(
	groups: StoryGroup[],
	ops: OperatorIndex,
	showSystem: boolean,
): StorySection[] {
	const sections = new Map<string, StorySection & { sort: number[] }>();
	for (const group of groups) {
		const m = /^([a-z]+?)(\d+)/.exec(group.id);
		const familyKey = m?.[1] ?? "";
		const num = m ? Number(m[2]) : 0;
		const family = FAMILIES[familyKey];
		const key = m ? `${familyKey}${num}` : "other";
		let section = sections.get(key);
		if (!section) {
			const head = family?.section(num, ops) ?? { title: "Other missions" };
			section = {
				key,
				title: head.title,
				icon: head.icon,
				family: family?.name,
				cards: [],
				sort: [family?.order ?? 99, num],
			};
			sections.set(key, section);
		}
		const card = groupCard(group, showSystem);
		if (card) section.cards.push(card);
	}
	return [...sections.values()]
		.filter((s) => s.cards.length > 0)
		.sort(
			(a, b) =>
				(a.sort[0] ?? 0) - (b.sort[0] ?? 0) ||
				(a.sort[1] ?? 0) - (b.sort[1] ?? 0),
		)
		.map(({ sort: _sort, ...section }) => {
			// Untitled side groups (`db01m2d1`) read better under their parent.
			section.cards.sort((a, b) => naturalCompare(a.key, b.key));
			return section;
		});
}

function groupCard(group: StoryGroup, showSystem: boolean): StoryCard | null {
	const dialogs = [...group.dialogs]
		.filter((d) => showSystem || !isSystemDialog(d))
		.sort((a, b) =>
			naturalCompare(dialogSuffix(a, group.id), dialogSuffix(b, group.id)),
		);
	if (dialogs.length === 0) return null;
	const labels = new Map(
		dialogs.map((d) => [d, partLabel(dialogSuffix(d, group.id))]),
	);
	return {
		key: group.id,
		title: group.title || untitled(group.id),
		dialogs,
		labels,
	};
}

/** `db01m2d1` → `Mission 2 · Side 1`, `gm01m23` → `Mission 23`. */
function untitled(groupId: string): string {
	const m = /m(\d+)(?:d(\d+))?$/.exec(groupId);
	if (!m) return "Untitled";
	return m[2] ? `Mission ${Number(m[1])} · Side ${m[2]}` : `Mission ${m[1]}`;
}

// ---------------------------------------------------------------- levels

function levelSections(groups: StoryGroup[], showSystem: boolean) {
	const sections = new Map<string, StorySection>();
	for (const group of groups) {
		const [area = "", spot = group.id] = group.id.split("/");
		const map = /^map(\d+)$/.exec(area);
		const key = area || "other";
		let section = sections.get(key);
		if (!section) {
			section = {
				key,
				title: map
					? `Map ${Number(map[1])}`
					: area === "indie"
						? "Standalone areas"
						: titleCase(area),
				family: "Levels",
				cards: [],
			};
			sections.set(key, section);
		}
		const card = groupCard(group, showSystem);
		if (!card) continue;
		const lv = /^lv(\d+)$/.exec(spot);
		const dg = /^dg(\d+)$/.exec(spot);
		card.title = lv
			? `Level ${Number(lv[1])}`
			: dg
				? `Dungeon ${Number(dg[1])}`
				: titleCase(spot);
		section.cards.push(card);
	}
	return [...sections.values()].sort((a, b) => naturalCompare(a.key, b.key));
}

// ---------------------------------------------------------------- flat categories

/** Splits a flat list (gameplay / NPC dialogs) into cards by id stem. */
function flatSections(
	group: StoryGroup,
	category: StoryCategoryId,
	ops: OperatorIndex,
	showSystem: boolean,
): StorySection[] {
	const cards = new Map<string, StoryCard>();
	for (const dialog of group.dialogs) {
		if (!showSystem && isSystemDialog(dialog)) continue;
		const { key, title, label, icon } =
			category === "npc"
				? npcParts(dialog, ops)
				: { ...gameplayParts(dialog), icon: undefined };
		let card = cards.get(key);
		if (!card) {
			card = { key, title, icon, dialogs: [], labels: new Map() };
			cards.set(key, card);
		}
		card.dialogs.push(dialog);
		card.labels.set(dialog, label);
	}
	const list = [...cards.values()];
	if (category === "npc") list.sort((a, b) => a.title.localeCompare(b.title));
	return list.length
		? [
				{
					key: category,
					title: category === "npc" ? "Operators on the ship" : "Base & AIC",
					cards: list,
				},
			]
		: [];
}

function npcParts(dialog: string, ops: OperatorIndex) {
	const m = /^dlg_npc_(\d{4})_([a-z0-9]+)_(.+)$/i.exec(dialog);
	if (!m) {
		const rest = dialog.replace(/^dlg_npc_/, "");
		return {
			key: rest,
			title: titleCase(rest),
			label: "Open",
			icon: undefined as string | undefined,
		};
	}
	const op = ops.get(Number(m[1]));
	return {
		key: `${m[1]}_${m[2]}`,
		title: op?.name ?? titleCase(m[2] ?? ""),
		icon: op?.icon ?? undefined,
		label: partLabel(m[3] ?? ""),
	};
}

function gameplayParts(dialog: string) {
	const body = dialog.replace(/^dlg_(gpl_|gameplay_)?/, "");
	// `blackbox_belt_2_1` → stem `blackbox_belt`, label `2-1`.
	const level = /^(.*?)_level_(\d+)$/.exec(body);
	if (level)
		return {
			key: level[1] ?? body,
			title: titleCase(level[1] ?? body),
			label: `Level ${level[2]}`,
		};
	const m = /^(.*?)((?:_\d+)+)$/.exec(body);
	const stem = m?.[1] ?? body;
	const nums = m?.[2]?.split("_").filter(Boolean) ?? [];
	const stemTitle = stem.startsWith("blackbox_")
		? `Blackbox · ${titleCase(stem.slice(9))}`
		: titleCase(stem);
	return {
		key: stem,
		title: stemTitle,
		label: nums.length ? `Part ${nums.join("-")}` : "Open",
	};
}

// ---------------------------------------------------------------- entry point

export function storySections(
	category: StoryCategoryId,
	groups: StoryGroup[],
	ops: OperatorIndex,
	showSystem: boolean,
): StorySection[] {
	if (category === "mission") return missionSections(groups, ops, showSystem);
	if (category === "level") return levelSections(groups, showSystem);
	return groups.flatMap((g) => flatSections(g, category, ops, showSystem));
}

/** Speaker names sometimes carry an unresolved `{token}`; drop it. */
export function cleanSpeakerName(name: string): string {
	return name.replace(/\s*\{[^}]*\}\s*/g, " ").trim() || name;
}

// ---------------------------------------------------------------- progress

const LAST_KEY = "fluorite.story.last";

export interface LastRead {
	id: string;
	title?: string;
	at: number;
}

export function saveLastRead(entry: LastRead): void {
	try {
		window.localStorage.setItem(LAST_KEY, JSON.stringify(entry));
	} catch {
		// Storage unavailable: nothing to resume later.
	}
}

export function loadLastRead(): LastRead | null {
	try {
		const raw = window.localStorage.getItem(LAST_KEY);
		if (!raw) return null;
		const v = JSON.parse(raw) as Partial<LastRead>;
		return typeof v.id === "string" && typeof v.at === "number"
			? { id: v.id, title: v.title, at: v.at }
			: null;
	} catch {
		return null;
	}
}
