/**
 * Catalog + story contract (API v1 "Catalog v1 shapes") and the query option
 * factories used by both route loaders and components.
 */
import { queryOptions } from "@tanstack/react-query";
import { z } from "zod";
import {
	CATALOG_KINDS,
	type CatalogKind,
	type CatalogModel,
	type CatalogTag,
	getJson,
} from "@/lib/api";
import type { AudioSources, VoiceLanguage } from "@/lib/audio";
import { DEFAULT_SETTINGS, SETTINGS_KEY } from "@/lib/settings";

const MINUTE = 60_000;

// ---------------------------------------------------------------- rich text

export interface RichSegment {
	text: string;
	/** Rich-text style id, e.g. `ba.vup`. */
	style?: string;
	/** Resolved `#rrggbb`. */
	color?: string;
	/** Glossary id; look it up in the response's `terms`. */
	term?: string;
	bold?: boolean;
	italic?: boolean;
	/** Inline sprite ref (text is empty). */
	image?: string;
	/** Input glyph id (text is empty). */
	action?: string;
}

export type Rich = RichSegment[];

export interface Term {
	name: string;
	description: Rich;
	icon?: string | null;
	color?: string | null;
}

export type Terms = Record<string, Term>;

/** Plain text of a rich value (for titles, search, aria labels). */
export function richToPlain(rich: Rich | string | null | undefined): string {
	if (!rich) return "";
	const text =
		typeof rich === "string" ? rich : rich.map((s) => s.text).join("");
	return text.replace(/<p="[^"]*"[^>]*>(.*?)<\/p>/g, "$1");
}

// ---------------------------------------------------------------- shared

export type Gender = "f" | "m";

export interface AttrSeries {
	level: number[];
	breakStage: number[];
	series: Record<string, number[]>;
	constants: Record<string, number>;
}

export interface Cost {
	id: string;
	count: number;
}

export interface NamedCode {
	code: string;
	name: string;
	icon?: string | null;
	color?: string | null;
}

export interface SkillLevel {
	level: number;
	description: Rich;
	blackboard?: Record<string, number> | null;
	stats?: { name: string; value: string }[] | null;
	cooldown?: number | null;
	cost?: { type: string; value: number } | null;
}

export interface Skill {
	id: string;
	type?: "NormalAttack" | "NormalSkill" | "ComboSkill" | "UltimateSkill";
	name: string;
	icon?: string | null;
	skillIds?: string[];
	levels: SkillLevel[];
}

// ---------------------------------------------------------------- list

export interface ListItem {
	id: string;
	name: string;
	rarity?: number | null;
	tags: CatalogTag[];
	icon?: string | null;
	hasModel: boolean;
}

export interface TypeFacet {
	code: string;
	name: string;
	count: number;
}

export interface CatalogListResponse {
	kind: CatalogKind;
	lang: string;
	total: number;
	offset: number;
	limit: number;
	items: ListItem[];
	types?: TypeFacet[] | null;
}

// ---------------------------------------------------------------- details

interface DetailBase {
	kind: CatalogKind;
	id: string;
	name: string;
	terms: Terms;
}

export interface OperatorArt {
	portrait?: string;
	gacha?: string;
	gachaShadow?: string;
	icon?: string;
	roundIcon?: string;
	horizontalHeadIcon?: string;
	remoteIcon?: string;
	battleIcon?: string;
	infoBackground?: string;
}

export interface OperatorDetail extends DetailBase {
	kind: "operators";
	engName?: string;
	rarity: number;
	profession: NamedCode;
	element: NamedCode;
	weaponType: NamedCode;
	department?: string | null;
	mainAttribute?: string | null;
	subAttribute?: string | null;
	battleTags: NamedCode[];
	defaultWeapon?: { id: string; name: string } | null;
	voiceActors: { cn?: string; en?: string; jp?: string; kr?: string };
	attributes: AttrSeries;
	promotions: {
		id: string;
		type: string;
		breakStage: number;
		name: string;
		description: Rich;
		cost: Cost[];
	}[];
	skills: Skill[];
	talents: {
		id: string;
		name: string;
		icon?: string | null;
		index: number;
		level: number;
		breakStage: number;
		effectId: string;
		description: Rich;
		cost: Cost[];
	}[];
	attributeNodes: {
		id: string;
		title: string;
		description: Rich;
		breakStage: number;
		modifiers: { attrType: string; value: number; modifierType: string }[];
		cost: Cost[];
	}[];
	potentials: {
		level: number;
		name: string;
		effectId: string;
		description: Rich;
	}[];
	profile: {
		id: string;
		title: string;
		text: Rich;
		unlock: { type: string; value: number };
	}[];
	voiceLines: {
		id: string;
		title: string;
		text: Rich;
		voId: string;
		/** Per voice language, when the API resolved the recording. */
		audio?: Partial<Record<VoiceLanguage, AudioSources>>;
	}[];
	art: OperatorArt;
	model?: CatalogModel | null;
}

export interface WeaponDetail extends DetailBase {
	kind: "weapons";
	rarity: number;
	weaponType: NamedCode;
	description: Rich;
	maxLevel: number;
	atk: { level: number[]; baseAtk: number[] };
	breakthroughs: { level: number; stage: number; gold: number; cost: Cost[] }[];
	skills: Skill[];
	art: { icon?: string; iconBig?: string; gacha?: string };
	model?: CatalogModel | null;
	/** Operators that use this weapon by default, when the API provides them. */
	operators?: { id: string; name: string }[] | null;
}

export interface Resistances {
	physical: number;
	fire: number;
	cryst: number;
	pulse: number;
	natural: number;
}

export interface EnemyTemplate {
	levels: AttrSeries;
	fixed: Record<string, number>;
	resistances: Resistances;
	maxResilience?: number | null;
}

export interface EnemyDetail extends DetailBase {
	kind: "enemies";
	nickname?: string | null;
	description: Rich;
	displayType: NamedCode;
	tags: NamedCode[];
	abilities: { id: string; name: string; description: Rich }[];
	variants: { id: string; name?: string | null; attrTemplateId: string }[];
	attributeTemplates: Record<string, EnemyTemplate>;
	art: { icon?: string; iconBig?: string; adventureIcon?: string };
	model?: CatalogModel | null;
}

export interface ItemDetail extends DetailBase {
	kind: "items";
	description: Rich;
	flavor: Rich;
	rarity: number;
	itemType: NamedCode;
	obtainWays: { id: string; description: string; system?: string | null }[];
	noObtainHint?: string | null;
	related?: { kind: CatalogKind; id: string } | null;
	art: { icon?: string; iconBig?: string };
}

export interface NpcDetail extends DetailBase {
	kind: "npcs";
	title?: string | null;
	faction?: string | null;
	dataKey?: string | null;
	groupId?: string | null;
	model?: CatalogModel | null;
}

export interface FacilityDetail extends DetailBase {
	kind: "facilities";
	description: Rich;
	type: string;
	powerConsume: number;
	needPower: boolean;
	bandwidth: number;
	range: { width: number; depth: number; height: number };
	inputPorts: number;
	outputPorts: number;
	art: { icon?: string; iconBig?: string; image?: string };
}

export type EntityDetail =
	| OperatorDetail
	| WeaponDetail
	| EnemyDetail
	| ItemDetail
	| NpcDetail
	| FacilityDetail;

export type DetailFor<K extends CatalogKind> = Extract<
	EntityDetail,
	{ kind: K }
>;

// ---------------------------------------------------------------- story

export type StoryCategoryId = "mission" | "level" | "gameplay" | "npc";

export interface StoryGroup {
	id: string;
	title?: string | null;
	dialogs: string[];
}

export interface StoryCategory {
	id: StoryCategoryId;
	count: number;
	groups: StoryGroup[];
}

export interface StoryList {
	lang: string;
	total: number;
	categories: StoryCategory[];
}

export interface DialogOption {
	id: string;
	text: Rich;
	next?: string | null;
}

export interface DialogNode {
	id: string;
	kind: "line" | "option" | "transition" | "finish";
	action?:
		| "transition"
		| "cinematic"
		| "dialog"
		| "summary"
		| "if"
		| "branch"
		| "empty"
		| null;
	speaker?: { name: string; id?: string | null; icon?: string | null } | null;
	text?: Rich | null;
	altText?: Rich | null;
	lineId?: string | null;
	emotion?: string | null;
	options?: DialogOption[] | null;
	ref?: string | null;
	next: string[];
}

export interface Dialog {
	id: string;
	category: StoryCategoryId;
	group: string;
	title?: string | null;
	lang: string;
	gender: Gender;
	start: string;
	nodes: DialogNode[];
}

// ---------------------------------------------------------------- labels

export const KIND_LABELS: Record<CatalogKind, string> = {
	operators: "Operators",
	weapons: "Weapons",
	enemies: "Enemies",
	items: "Items",
	npcs: "NPCs",
	facilities: "Facilities",
};

export const KIND_SINGULAR: Record<CatalogKind, string> = {
	operators: "Operator",
	weapons: "Weapon",
	enemies: "Enemy",
	items: "Item",
	npcs: "NPC",
	facilities: "Facility",
};

export function isCatalogKind(value: string): value is CatalogKind {
	return (CATALOG_KINDS as readonly string[]).includes(value);
}

/** Display order for kinds (sidebar / palette / overview). */
export const KIND_ORDER: CatalogKind[] = [
	"operators",
	"weapons",
	"enemies",
	"items",
	"npcs",
	"facilities",
];

export const TAG_KIND_LABELS: Record<string, string> = {
	profession: "Class",
	element: "Element",
	weaponType: "Weapon",
	displayType: "Tier",
	enemyTag: "Tag",
	itemType: "Type",
	title: "Title",
	faction: "Faction",
	buildingType: "Type",
};

export const STORY_CATEGORY_LABELS: Record<StoryCategoryId, string> = {
	mission: "Missions",
	level: "Levels",
	gameplay: "Gameplay",
	npc: "NPCs",
};

const ATTR_LABELS: Record<string, string> = {
	Agi: "Agility",
	Str: "Strength",
	Wisd: "Intellect",
	Will: "Will",
	Atk: "ATK",
	Def: "DEF",
	MaxHp: "HP",
	MaxPoise: "Max Stagger",
	CriticalRate: "Crit Rate",
	CriticalDamageIncrease: "Crit DMG",
	AttackRate: "Attack Speed",
	Hatred: "Aggro",
	NormalAttackRange: "Basic ATK Range",
	NormalAttackStartRange: "Basic ATK Start Range",
	PoiseRecTime: "Stagger Recovery",
};

/** Human label for an attribute code (`MaxHp` → `HP`, `FireDamageIncrease` → `Fire Damage Increase`). */
export function attrLabel(code: string): string {
	return (
		ATTR_LABELS[code] ??
		code
			.replace(/([a-z])([A-Z])/g, "$1 $2")
			.replace(/Cryst/g, "Cryo")
			.replace(/Natural/g, "Nature")
			.replace(/Pulse/g, "Electric")
			.replace(/Ether/g, "Arts")
	);
}

/** Primary attributes first, then the rest alphabetically. */
const ATTR_ORDER = ["MaxHp", "Atk", "Def", "Str", "Agi", "Wisd", "Will"];
export function sortAttrs(codes: string[]): string[] {
	return [...codes].sort((a, b) => {
		const ia = ATTR_ORDER.indexOf(a);
		const ib = ATTR_ORDER.indexOf(b);
		if (ia !== -1 || ib !== -1) {
			return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
		}
		return a.localeCompare(b);
	});
}

/** Rarity accent colours (game palette). */
export const RARITY_COLORS: Record<number, string> = {
	1: "#9aa0a6",
	2: "#8fc75a",
	3: "#3fa7f5",
	4: "#a874f0",
	5: "#ffc43d",
	6: "#ff7a1a",
};

export function rarityColor(
	rarity: number | null | undefined,
): string | undefined {
	return rarity ? RARITY_COLORS[rarity] : undefined;
}

// ---------------------------------------------------------------- language

/**
 * Text language for loaders. The server has no access to the viewer's
 * localStorage, so SSR renders the default language and the client refetches.
 */
export function loaderLanguage(): string {
	if (typeof window === "undefined") return DEFAULT_SETTINGS.textLanguage;
	try {
		const raw = window.localStorage.getItem(SETTINGS_KEY);
		const parsed = raw ? (JSON.parse(raw) as { textLanguage?: unknown }) : {};
		return typeof parsed.textLanguage === "string" && parsed.textLanguage
			? parsed.textLanguage
			: DEFAULT_SETTINGS.textLanguage;
	} catch {
		return DEFAULT_SETTINGS.textLanguage;
	}
}

// ---------------------------------------------------------------- queries

export const LIST_LIMIT = 5000;

export const catalogListQuery = (kind: CatalogKind, lang: string) =>
	queryOptions({
		queryKey: ["catalog", kind, lang, "list"],
		queryFn: ({ signal }) =>
			getJson<CatalogListResponse>(
				`/api/catalog/${kind}`,
				{ lang, limit: LIST_LIMIT },
				signal,
			),
		staleTime: 10 * MINUTE,
	});

export const entityQuery = <K extends CatalogKind>(
	kind: K,
	id: string,
	lang: string,
	gender: Gender = "f",
) =>
	queryOptions({
		queryKey: ["catalog", kind, lang, "detail", id, gender],
		queryFn: ({ signal }) =>
			getJson<DetailFor<K>>(
				`/api/catalog/${kind}/${encodeURIComponent(id)}`,
				{ lang, gender: gender === "f" ? undefined : gender },
				signal,
			),
		staleTime: 10 * MINUTE,
	});

export interface CatalogHit extends ListItem {
	kind: CatalogKind;
}

/** Name search across every kind (command palette). */
export const catalogSearchQuery = (q: string, lang: string, perKind = 6) =>
	queryOptions({
		queryKey: ["catalog-search", lang, q, perKind],
		queryFn: async ({ signal }) => {
			const lists = await Promise.all(
				KIND_ORDER.map((kind) =>
					getJson<CatalogListResponse>(
						`/api/catalog/${kind}`,
						{ lang, q, limit: perKind },
						signal,
					).catch(() => null),
				),
			);
			const hits: { kind: CatalogKind; total: number; items: CatalogHit[] }[] =
				[];
			lists.forEach((list, i) => {
				const kind = KIND_ORDER[i];
				if (!list || !kind || list.items.length === 0) return;
				hits.push({
					kind,
					total: list.total,
					items: list.items.map((item) => ({ ...item, kind })),
				});
			});
			return hits;
		},
		staleTime: 5 * MINUTE,
		enabled: q.trim().length > 0,
	});

export const storyListQuery = (lang: string) =>
	queryOptions({
		queryKey: ["story", lang, "list"],
		queryFn: ({ signal }) => getJson<StoryList>("/api/story", { lang }, signal),
		staleTime: 10 * MINUTE,
	});

export const dialogQuery = (id: string, lang: string, gender: Gender = "f") =>
	queryOptions({
		queryKey: ["story", lang, "dialog", id, gender],
		queryFn: ({ signal }) =>
			getJson<Dialog>(
				`/api/story/${encodeURIComponent(id)}`,
				{ lang, gender: gender === "f" ? undefined : gender },
				signal,
			),
		staleTime: 10 * MINUTE,
	});

// ---------------------------------------------------------------- story helpers

/** `dlg_a1m2_3` in group `a1m2` → `3`; falls back to the id without `dlg_`. */
export function dialogShortLabel(dialogId: string, groupId: string): string {
	const prefix = `dlg_${groupId.replace(/\//g, "_")}_`;
	if (groupId && dialogId.startsWith(prefix))
		return dialogId.slice(prefix.length);
	return dialogId.replace(/^dlg_/, "");
}

/** Finds the category/group a dialog belongs to in the story list. */
export function locateDialog(
	list: StoryList | undefined,
	dialogId: string,
): { category: StoryCategory; group: StoryGroup; index: number } | null {
	if (!list) return null;
	for (const category of list.categories) {
		for (const group of category.groups) {
			const index = group.dialogs.indexOf(dialogId);
			if (index !== -1) return { category, group, index };
		}
	}
	return null;
}

// ---------------------------------------------------------------- search params

/**
 * Optional string search param. The router JSON-parses values, so `?q=130`
 * or `?line=17` arrive as numbers; accept both and keep strings.
 */
export const searchText = z
	.union([z.string(), z.number()])
	.transform(String)
	.optional()
	.catch(undefined);
