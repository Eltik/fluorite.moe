/**
 * Character-screen props: the operator's default weapon
 * (`charinfo/weapons/<id>.json`) and the deco meshes
 * (`charinfo/deco/<id>/index.json`), with the per-clip visibility tracks the
 * exporter pulled out of the game's animator curves. Pure data helpers; the
 * three.js side lives in `components/studio/charscreen.ts`.
 *
 * Tracks are step functions `[[time, 0|1], …]` in clip seconds (1 = shown).
 */

export type VisibilityTrack = [number, number][];

export interface WeaponMount {
	node: string;
	scale: number;
}

export interface WeaponInfo {
	id: string;
	weapon: string;
	type: string;
	rarity: number;
	/** GLB path under the models root, or null when it was not exported. */
	glb: string | null;
	mounts: WeaponMount[];
	unresolvedMounts: string[];
	/** Visibility per clip file. Clips without a track hide the weapon. */
	clips: Record<string, VisibilityTrack>;
	/** Clips that always hide the weapon (pose props). */
	hideWeapon: string[];
}

export interface DecoItem {
	n: number;
	prefab: string;
	/** `<n>.glb` inside the deco folder, or null when it was not exported. */
	file: string | null;
	/** Node of the character to attach to; empty = the character root. */
	mount: string;
	kind: string;
}

/** Per item (`n`): visibility tracks per node path inside the prop. */
export type DecoClipTracks = Record<
	string,
	{ visible: Record<string, VisibilityTrack> }
>;

export interface DecoIndex {
	id: string;
	items: DecoItem[];
	/** Keyed by clip file. Clips without an entry hide every prop. */
	clips: Record<string, DecoClipTracks>;
}

function num(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value)
		? value
		: undefined;
}

function str(value: unknown): string | undefined {
	return typeof value === "string" && value ? value : undefined;
}

function strings(value: unknown): string[] {
	return Array.isArray(value)
		? value.filter((v): v is string => typeof v === "string")
		: [];
}

function track(value: unknown): VisibilityTrack | null {
	if (!Array.isArray(value)) return null;
	const keys: VisibilityTrack = [];
	for (const k of value) {
		if (!Array.isArray(k) || k.length < 2) continue;
		const t = num(k[0]);
		const v = num(k[1]);
		if (t === undefined || v === undefined) continue;
		keys.push([t, v]);
	}
	keys.sort((a, b) => a[0] - b[0]);
	return keys.length ? keys : null;
}

function tracks(value: unknown): Record<string, VisibilityTrack> {
	const out: Record<string, VisibilityTrack> = {};
	if (!value || typeof value !== "object") return out;
	for (const [key, raw] of Object.entries(value as Record<string, unknown>)) {
		const t = track(raw);
		if (t) out[key] = t;
	}
	return out;
}

/** Validates a weapon JSON; null when unusable. */
export function parseWeaponInfo(json: unknown): WeaponInfo | null {
	if (!json || typeof json !== "object") return null;
	const r = json as Record<string, unknown>;
	const mounts: WeaponMount[] = [];
	if (Array.isArray(r.mounts)) {
		for (const raw of r.mounts) {
			if (!raw || typeof raw !== "object") continue;
			const m = raw as Record<string, unknown>;
			const node = str(m.node);
			if (!node) continue;
			const scale = num(m.scale);
			mounts.push({ node, scale: scale && scale > 0 ? scale : 1 });
		}
	}
	return {
		id: str(r.id) ?? "",
		weapon: str(r.weapon) ?? "",
		type: str(r.type) ?? "",
		rarity: num(r.rarity) ?? 0,
		glb: str(r.glb) ?? null,
		mounts,
		unresolvedMounts: strings(r.unresolvedMounts),
		clips: tracks(r.clips),
		hideWeapon: strings(r.hideWeapon),
	};
}

/** Validates a deco index; null when unusable. */
export function parseDecoIndex(json: unknown): DecoIndex | null {
	if (!json || typeof json !== "object") return null;
	const r = json as Record<string, unknown>;
	const items: DecoItem[] = [];
	if (Array.isArray(r.items)) {
		for (const raw of r.items) {
			if (!raw || typeof raw !== "object") continue;
			const it = raw as Record<string, unknown>;
			const n = num(it.n);
			if (n === undefined) continue;
			items.push({
				n,
				prefab: str(it.prefab) ?? `deco_${n}`,
				file: str(it.file) ?? null,
				mount: typeof it.mount === "string" ? it.mount : "",
				kind: str(it.kind) ?? "deco",
			});
		}
	}
	const clips: Record<string, DecoClipTracks> = {};
	if (r.clips && typeof r.clips === "object") {
		for (const [file, rawItems] of Object.entries(
			r.clips as Record<string, unknown>,
		)) {
			if (!rawItems || typeof rawItems !== "object") continue;
			const perItem: DecoClipTracks = {};
			for (const [n, rawItem] of Object.entries(
				rawItems as Record<string, unknown>,
			)) {
				const visible = tracks(
					(rawItem as Record<string, unknown> | null)?.visible,
				);
				perItem[n] = { visible };
			}
			clips[file] = perItem;
		}
	}
	return { id: str(r.id) ?? "", items, clips };
}

/**
 * Samples a step track: the value of the last key at or before `time`, the
 * first key's value before the track starts.
 */
export function sampleTrack(track: VisibilityTrack, time: number): number {
	let value = track[0][1];
	for (const [t, v] of track) {
		if (t > time) break;
		value = v;
	}
	return value;
}

/** Clip sources whose clips hold the weapon even without a UI track. */
export type WeaponClipSource =
	| "ui"
	| "battle"
	| "3c"
	| "interact"
	| "dialog"
	| "other";

/**
 * Whether the default weapon shows during `clip` at `time`. UI clips follow
 * their exported track (hidden when the clip has none: the overview loop
 * never draws it), `hideWeapon` clips always hide it, and battle clips show
 * it since the game fights with it drawn.
 */
export function weaponVisible(
	info: WeaponInfo,
	clip: string | null,
	source: WeaponClipSource | null,
	time: number,
): boolean {
	if (!clip) return false;
	if (info.hideWeapon.includes(clip)) return false;
	const t = info.clips[clip];
	if (t) return sampleTrack(t, time) >= 0.5;
	return source === "battle";
}

/**
 * Per-node visibility of one deco item during `clip`: node path → shown.
 * An empty map means the whole item is hidden (no entry for this clip).
 */
export function decoVisibility(
	index: DecoIndex,
	item: DecoItem,
	clip: string | null,
	time: number,
): Record<string, boolean> {
	if (!clip) return {};
	const perItem = index.clips[clip]?.[String(item.n)];
	if (!perItem) return {};
	const out: Record<string, boolean> = {};
	for (const [path, t] of Object.entries(perItem.visible)) {
		out[path] = sampleTrack(t, time) >= 0.5;
	}
	return out;
}

/** The last path segment of a node path (`a/b/Mesh_all` → `Mesh_all`). */
export function nodeName(path: string): string {
	const i = path.lastIndexOf("/");
	return i >= 0 ? path.slice(i + 1) : path;
}
