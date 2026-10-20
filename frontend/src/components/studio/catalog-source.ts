/**
 * Studio data source: the catalog API, with a mesh-folder fallback for when a
 * catalog is not mapped yet (the API answers 501). The fallback lists the
 * entity folders under `meshes/beyond/arts/entity/…` and picks the best LOD
 * per part, so the Studio stays usable before the catalogs land.
 */
import { queryOptions } from "@tanstack/react-query";
import {
	api,
	type CatalogDetail,
	type CatalogItem,
	isApiError,
	type SearchItem,
} from "@/lib/api";
import type { StudioKind } from "@/lib/studio";

const MINUTE = 60_000;

export interface StudioEntry {
	kind: StudioKind;
	id: string;
	name: string;
	rarity?: number;
	tags: string[];
	/** Icon path under the `textures` root. */
	icon?: string;
	/** Mesh folder, known only for fallback entries. */
	folder?: string;
}

export interface StudioCatalog {
	items: StudioEntry[];
	source: "api" | "meshes";
}

export interface StudioModelSource {
	entry: StudioEntry;
	description?: string;
	/** Rigged GLB under the `models` root. */
	rigged?: string;
	/** Static part GLBs under the `meshes` root. */
	parts: string[];
	source: "api" | "meshes";
}

// ------------------------------------------------------------- fallback

const ENTITY_ROOT = "beyond/arts/entity";

const FALLBACK_BASE: Record<StudioKind, string> = {
	operators: `${ENTITY_ROOT}/actor`,
	npcs: `${ENTITY_ROOT}/npc/major`,
	enemies: `${ENTITY_ROOT}/monster`,
	weapons: `${ENTITY_ROOT}/wpn`,
};

const SKIP_GROUPS = new Set(["animations", "common", "shared"]);

function titleCase(code: string): string {
	return code
		.split(/[_\s]+/)
		.filter(Boolean)
		.map((w) => w.charAt(0).toUpperCase() + w.slice(1))
		.join(" ");
}

async function fallbackCatalog(
	kind: StudioKind,
	signal?: AbortSignal,
): Promise<StudioEntry[]> {
	const base = FALLBACK_BASE[kind];
	const groups = await api.tree("meshes", { path: base, limit: 1000 }, signal);
	const lists = await Promise.all(
		groups.entries
			.filter((g) => g.kind === "dir" && !SKIP_GROUPS.has(g.name))
			.map(async (group) => {
				const page = await api.tree(
					"meshes",
					{ path: group.path, limit: 2000 },
					signal,
				);
				return page.entries
					.filter((e) => e.kind === "dir" && !SKIP_GROUPS.has(e.name))
					.map<StudioEntry>((e) => ({
						kind,
						id: e.name,
						name: kind === "weapons" ? e.name : titleCase(e.name),
						tags: [group.name],
						folder: e.path,
					}));
			}),
	);
	// The same id can appear in several folders; keep the first.
	const seen = new Set<string>();
	return lists
		.flat()
		.filter((e) => !seen.has(e.id) && Boolean(seen.add(e.id)))
		.sort((a, b) => a.name.localeCompare(b.name));
}

/** `chr_0005_chen` → `chen`; plain codes pass through. */
function codeFromId(kind: StudioKind, id: string): string {
	if (kind === "weapons") return id;
	const m = /^(?:chr|npc|eny)_\d+_(.+)$/i.exec(id);
	return (m ? m[1] : id).toLowerCase();
}

/** Whether a picker entry is the model at `/studio/$kind/$id`. */
export function entryMatches(
	entry: StudioEntry,
	kind: StudioKind | undefined,
	id: string | undefined,
): boolean {
	if (!kind || !id || entry.kind !== kind) return false;
	if (entry.id === id) return true;
	// Mesh-folder entries are keyed by code; URLs may carry the full game id.
	return (
		Boolean(entry.folder) && entry.id.toLowerCase() === codeFromId(kind, id)
	);
}

const LOD_RE = /_lod(\d+)\.glb$/i;

/** Keeps the lowest LOD per part, preserving name order. */
export function bestLodParts(paths: string[]): string[] {
	const best = new Map<string, { path: string; lod: number }>();
	for (const path of paths) {
		if (!path.toLowerCase().endsWith(".glb")) continue;
		const m = LOD_RE.exec(path);
		const key = m ? path.slice(0, m.index) : path;
		const lod = m ? Number(m[1]) : 0;
		const prev = best.get(key);
		if (!prev || lod < prev.lod) best.set(key, { path, lod });
	}
	return [...best.values()]
		.map((v) => v.path)
		.sort((a, b) => a.localeCompare(b));
}

async function fallbackDetail(
	kind: StudioKind,
	id: string,
	catalog: StudioEntry[],
	signal?: AbortSignal,
): Promise<StudioModelSource | null> {
	const code = codeFromId(kind, id);
	const entry =
		catalog.find((e) => e.id === id) ??
		catalog.find((e) => e.id.toLowerCase() === code);
	if (!entry?.folder) return null;
	const prefix = `${entry.folder}/`;
	const items: SearchItem[] = [];
	for (let offset = 0; offset < 4000; offset += 500) {
		const page = await api.search(
			{ q: prefix, root: "meshes", ext: "glb", limit: 500, offset },
			signal,
		);
		items.push(...page.items);
		if (page.items.length < 500) break;
	}
	// Only the entity's own models folder: skip dialog/battle variants.
	const own = items
		.map((i) => i.path)
		.filter((p) => p.startsWith(prefix))
		.filter((p) => {
			const rest = p.slice(prefix.length).split("/");
			return rest.length === 1 || (rest.length === 2 && rest[0] === "models");
		});
	return {
		entry: { ...entry, kind },
		parts: bestLodParts(own),
		source: "meshes",
	};
}

// ------------------------------------------------------------- api mapping

function toEntry(kind: StudioKind, item: CatalogItem): StudioEntry {
	return {
		kind,
		id: item.id,
		name: item.name || item.id,
		rarity: item.rarity,
		tags: (item.tags ?? [])
			.map((t) => (typeof t === "string" ? t : t?.name))
			.filter((t): t is string => Boolean(t)),
		icon: item.icon,
	};
}

function str(value: unknown): string | undefined {
	return typeof value === "string" && value ? value : undefined;
}

/** Part paths from `["a.glb"]` or `[{ path: "a.glb" }]`. */
function strings(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value
		.map((v) =>
			typeof v === "string"
				? v
				: v && typeof v === "object" && typeof v.path === "string"
					? (v.path as string)
					: null,
		)
		.filter((v): v is string => Boolean(v));
}

/**
 * Pulls model references out of a detail payload. Accepts `model.parts` /
 * `model.rigged`, top-level `rigged`, and a `models` object or array, since
 * the detail shape is kind-specific.
 */
export function modelFromDetail(detail: CatalogDetail): {
	parts: string[];
	rigged?: string;
} {
	const record = detail as Record<string, unknown>;
	const model = (record.model ?? {}) as Record<string, unknown>;
	const models = record.models;
	let parts = strings(model.parts);
	let rigged = str(model.rigged) ?? str(record.rigged);
	if (models && typeof models === "object") {
		if (Array.isArray(models)) {
			for (const m of models) {
				if (!m || typeof m !== "object") continue;
				const mm = m as Record<string, unknown>;
				if (!parts.length) parts = strings(mm.parts);
				rigged ??= str(mm.rigged);
			}
		} else {
			const mm = models as Record<string, unknown>;
			if (!parts.length) parts = strings(mm.parts);
			rigged ??= str(mm.rigged);
		}
	}
	return { parts: bestLodParts(parts), rigged };
}

// ------------------------------------------------------------- queries

export const studioCatalogQuery = (kind: StudioKind, lang: string) =>
	queryOptions({
		queryKey: ["studio", "catalog", kind, lang],
		queryFn: async ({ signal }): Promise<StudioCatalog> => {
			try {
				const list = await api.catalog(kind, lang, signal);
				return {
					// Only entries with a model belong in the Studio picker.
					items: list.items
						.filter((item) => item.hasModel !== false)
						.map((item) => toEntry(kind, item)),
					source: "api",
				};
			} catch (error) {
				if (isApiError(error) && error.isNotImplemented) {
					return {
						items: await fallbackCatalog(kind, signal),
						source: "meshes",
					};
				}
				throw error;
			}
		},
		staleTime: 10 * MINUTE,
		retry: false,
	});

export class UnknownModelError extends Error {
	constructor(kind: StudioKind, id: string) {
		super(`Unknown model id: ${id} (${kind})`);
		this.name = "UnknownModelError";
	}
}

export const studioDetailQuery = (
	kind: StudioKind,
	id: string,
	lang: string,
	getCatalog: () => Promise<StudioCatalog>,
) =>
	queryOptions({
		queryKey: ["studio", "detail", kind, id, lang],
		queryFn: async ({ signal }): Promise<StudioModelSource> => {
			try {
				const detail = await api.catalogDetail(kind, id, lang, signal);
				const { parts, rigged } = modelFromDetail(detail);
				return {
					entry: toEntry(kind, detail),
					description: str(detail.description),
					parts,
					rigged,
					source: "api",
				};
			} catch (error) {
				if (isApiError(error) && error.isNotFound) {
					throw new UnknownModelError(kind, id);
				}
				if (isApiError(error) && error.isNotImplemented) {
					const catalog = await getCatalog();
					const source = await fallbackDetail(kind, id, catalog.items, signal);
					if (!source) throw new UnknownModelError(kind, id);
					return source;
				}
				throw error;
			}
		},
		staleTime: 10 * MINUTE,
		retry: false,
	});
