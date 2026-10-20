/**
 * Typed client for the fluorite-api v1 contract.
 *
 * Every request goes through `request()`, which turns the API's error envelope
 * (`{ error: { code, message } }`) into an `ApiError` so query error states can
 * branch on `status` / `code` (e.g. 501 for catalogs that are not mapped yet).
 */
import { parseJsonSafe } from "@/lib/json-parse";

export const API_URL: string = (
	import.meta.env.VITE_API_URL ?? "http://127.0.0.1:8787"
).replace(/\/+$/, "");

export const ROOTS = [
	"textures",
	"meshes",
	"gamedata",
	"tables",
	"jsondata",
	"lua",
	"audio",
] as const;
export type Root = (typeof ROOTS)[number];

export function isRoot(value: string): value is Root {
	return (ROOTS as readonly string[]).includes(value);
}

export const CATALOG_KINDS = [
	"operators",
	"enemies",
	"weapons",
	"items",
	"npcs",
	"facilities",
] as const;
export type CatalogKind = (typeof CATALOG_KINDS)[number];

/** Languages with an `I18nTextTable_<LANG>.json`; used until the API answers. */
export const TEXT_LANGUAGES = [
	"EN",
	"CN",
	"TC",
	"JP",
	"KR",
	"BR",
	"DE",
	"FR",
	"ID",
	"IT",
	"MX",
	"RU",
	"TH",
	"VN",
] as const;
export type TextLanguage = (typeof TEXT_LANGUAGES)[number];

// ---------------------------------------------------------------- types

export interface StatusCounts {
	textures: number;
	meshes: number;
	tables: number;
	jsondata: number;
	lua: number;
	gamedata: number;
}

export interface Status {
	version: string;
	resVersion: string;
	updatedAt: string;
	schemaVersion: number;
	counts: StatusCounts;
}

export interface TreeEntry {
	name: string;
	path: string;
	kind: "dir" | "file";
	size?: number;
	children?: number;
}

export interface TreePage {
	root: Root;
	path: string;
	entries: TreeEntry[];
	total: number;
	offset: number;
	limit: number;
}

export type SearchExt = "png" | "glb" | "json" | "lua";

export interface SearchItem {
	root: Root;
	path: string;
	name: string;
	size: number;
}

export interface SearchResult {
	query: string;
	total: number;
	items: SearchItem[];
}

export interface TableInfo {
	name: string;
	size: number;
}

export interface CatalogItem {
	id: string;
	name: string;
	rarity?: number;
	tags: string[];
	/** Path relative to the `textures` root. */
	icon?: string;
	model?: { parts: string[] };
}

export interface CatalogList {
	kind: CatalogKind;
	items: CatalogItem[];
}

/** Kind-specific detail; the shape is not pinned down by the contract yet. */
export interface CatalogDetail extends CatalogItem {
	description?: string;
	[key: string]: unknown;
}

// ---------------------------------------------------------------- errors

export class ApiError extends Error {
	readonly status: number;
	readonly code: string;

	constructor(status: number, code: string, message: string) {
		super(message);
		this.name = "ApiError";
		this.status = status;
		this.code = code;
	}

	get isNotImplemented(): boolean {
		return this.status === 501;
	}

	get isNotFound(): boolean {
		return this.status === 404;
	}
}

export function isApiError(error: unknown): error is ApiError {
	return error instanceof ApiError;
}

/** True for failures where the API itself could not be reached. */
export function isNetworkError(error: unknown): boolean {
	return (
		error instanceof TypeError || (isApiError(error) && error.status === 0)
	);
}

// ---------------------------------------------------------------- core

type Query = Record<string, string | number | undefined | null>;

function buildUrl(path: string, query?: Query): string {
	const url = new URL(`${API_URL}${path}`);
	if (query) {
		for (const [k, v] of Object.entries(query)) {
			if (v !== undefined && v !== null && v !== "") {
				url.searchParams.set(k, String(v));
			}
		}
	}
	return url.toString();
}

async function toApiError(res: Response): Promise<ApiError> {
	let code = res.status === 501 ? "not_implemented" : "internal";
	let message = res.statusText || `Request failed (${res.status})`;
	try {
		const body = (await res.json()) as {
			error?: { code?: string; message?: string };
		};
		if (body.error?.code) code = body.error.code;
		if (body.error?.message) message = body.error.message;
	} catch {
		// Non-JSON error body; keep the status text.
	}
	return new ApiError(res.status, code, message);
}

async function send(
	path: string,
	init: RequestInit & { query?: Query } = {},
): Promise<Response> {
	const { query, ...rest } = init;
	let res: Response;
	try {
		res = await fetch(buildUrl(path, query), rest);
	} catch (cause) {
		const error = new ApiError(
			0,
			"unreachable",
			`Could not reach the fluorite API at ${API_URL}`,
		);
		error.cause = cause;
		throw error;
	}
	if (!res.ok) throw await toApiError(res);
	return res;
}

async function request<T>(
	path: string,
	init?: RequestInit & { query?: Query },
): Promise<T> {
	const res = await send(path, init);
	return (await res.json()) as T;
}

/** Encodes each segment of a relative path, keeping the slashes. */
export function encodePath(path: string): string {
	return path.split("/").filter(Boolean).map(encodeURIComponent).join("/");
}

// ---------------------------------------------------------------- endpoints

export const api = {
	status(signal?: AbortSignal): Promise<Status> {
		return request("/api/status", { signal });
	},

	tree(
		root: Root,
		params: { path?: string; offset?: number; limit?: number } = {},
		signal?: AbortSignal,
	): Promise<TreePage> {
		return request(`/api/tree/${root}`, {
			query: { path: params.path, offset: params.offset, limit: params.limit },
			signal,
		});
	},

	search(
		params: {
			q: string;
			root?: Root;
			ext?: SearchExt;
			limit?: number;
			offset?: number;
		},
		signal?: AbortSignal,
	): Promise<SearchResult> {
		return request("/api/search", { query: params, signal });
	},

	tables(signal?: AbortSignal): Promise<{ tables: TableInfo[] }> {
		return request("/api/tables", { signal });
	},

	/** Table JSON, parsed with 64-bit integer ids preserved as strings. */
	async table(name: string, signal?: AbortSignal): Promise<unknown> {
		const res = await send(`/api/tables/${encodeURIComponent(name)}`, {
			signal,
		});
		return parseJsonSafe(await res.text());
	},

	i18nLanguages(signal?: AbortSignal): Promise<{ languages: string[] }> {
		return request("/api/i18n/languages", { signal });
	},

	async i18nText(
		lang: string,
		key: string,
		signal?: AbortSignal,
	): Promise<string> {
		const body = await request<{ text?: string } | string>(
			`/api/i18n/${encodeURIComponent(lang)}/${encodeURIComponent(key)}`,
			{ signal },
		);
		return typeof body === "string" ? body : (body.text ?? "");
	},

	i18nResolve(
		lang: string,
		keys: string[],
		signal?: AbortSignal,
	): Promise<{ texts: Record<string, string> }> {
		return request(`/api/i18n/${encodeURIComponent(lang)}/resolve`, {
			method: "POST",
			headers: { "content-type": "application/json" },
			body: JSON.stringify({ keys }),
			signal,
		});
	},

	/** Raw file body as text (JSON / Lua / plain text previews). */
	async fileText(
		root: Root,
		path: string,
		signal?: AbortSignal,
	): Promise<string> {
		const res = await send(`/files/${root}/${encodePath(path)}`, { signal });
		return res.text();
	},

	catalog(
		kind: CatalogKind,
		lang: string,
		signal?: AbortSignal,
	): Promise<CatalogList> {
		return request(`/api/catalog/${kind}`, { query: { lang }, signal });
	},

	catalogDetail(
		kind: CatalogKind,
		id: string,
		lang: string,
		signal?: AbortSignal,
	): Promise<CatalogDetail> {
		return request(`/api/catalog/${kind}/${encodeURIComponent(id)}`, {
			query: { lang },
			signal,
		});
	},
};

// ---------------------------------------------------------------- URL builders

export function fileUrl(root: Root, path: string): string {
	return `${API_URL}/files/${root}/${encodePath(path)}`;
}

export type ThumbSize = 128 | 256 | 512;

export function thumbUrl(path: string, size: ThumbSize = 256): string {
	return `${API_URL}/thumb/${encodePath(path)}?size=${size}`;
}
