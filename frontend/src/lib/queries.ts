/**
 * Query option factories shared by loaders (`queryClient.query`) and
 * components (`useQuery` / `useSuspenseQuery`), so both use identical keys.
 */
import { queryOptions } from "@tanstack/react-query";
import {
	api,
	type CatalogKind,
	type Root,
	type SearchExt,
	TEXT_LANGUAGES,
} from "@/lib/api";

const MINUTE = 60_000;

export const statusQuery = () =>
	queryOptions({
		queryKey: ["status"],
		queryFn: ({ signal }) => api.status(signal),
		staleTime: MINUTE,
		refetchInterval: 5 * MINUTE,
		retry: 1,
	});

/** Directory listings are capped well above any real folder size. */
export const TREE_LIMIT = 20_000;

export const treeQuery = (root: Root, path: string) =>
	queryOptions({
		queryKey: ["tree", root, path],
		queryFn: ({ signal }) =>
			api.tree(root, { path, limit: TREE_LIMIT }, signal),
		staleTime: 5 * MINUTE,
	});

export const searchQuery = (params: {
	q: string;
	root?: Root;
	ext?: SearchExt;
	limit?: number;
}) =>
	queryOptions({
		queryKey: ["search", params],
		queryFn: ({ signal }) => api.search(params, signal),
		staleTime: 5 * MINUTE,
		enabled: params.q.trim().length > 0,
	});

export const tablesQuery = () =>
	queryOptions({
		queryKey: ["tables"],
		queryFn: ({ signal }) => api.tables(signal),
		staleTime: 10 * MINUTE,
	});

/** Table bodies can be many MB: fetched client-side only, kept briefly. */
export const tableQuery = (name: string) =>
	queryOptions({
		queryKey: ["table", name],
		queryFn: ({ signal }) => api.table(name, signal),
		staleTime: 10 * MINUTE,
		gcTime: 2 * MINUTE,
	});

export const fileTextQuery = (root: Root, path: string) =>
	queryOptions({
		queryKey: ["file-text", root, path],
		queryFn: ({ signal }) => api.fileText(root, path, signal),
		staleTime: 10 * MINUTE,
		gcTime: 2 * MINUTE,
	});

export const languagesQuery = () =>
	queryOptions({
		queryKey: ["i18n", "languages"],
		queryFn: async ({ signal }) => {
			try {
				return (await api.i18nLanguages(signal)).languages;
			} catch {
				return [...TEXT_LANGUAGES] as string[];
			}
		},
		staleTime: Number.POSITIVE_INFINITY,
	});

export const i18nResolveQuery = (lang: string, keys: string[]) =>
	queryOptions({
		queryKey: ["i18n", "resolve", lang, keys],
		queryFn: ({ signal }) => api.i18nResolve(lang, keys, signal),
		staleTime: Number.POSITIVE_INFINITY,
		enabled: keys.length > 0,
	});

/** Entity counts per catalog kind (sidebar badges, home quick links). */
export const catalogKindsQuery = () =>
	queryOptions({
		queryKey: ["catalog", "kinds"],
		queryFn: async ({ signal }) => {
			const { kinds } = await api.catalogKinds(signal);
			return Object.fromEntries(kinds.map((k) => [k.kind, k.count])) as Partial<
				Record<CatalogKind, number>
			>;
		},
		staleTime: 10 * MINUTE,
		retry: 1,
	});

export const catalogQuery = (kind: CatalogKind, lang: string) =>
	queryOptions({
		queryKey: ["catalog", kind, lang],
		queryFn: ({ signal }) => api.catalog(kind, lang, signal),
		staleTime: 10 * MINUTE,
		retry: (count, error) =>
			!(error instanceof Error && "status" in error && error.status === 501) &&
			count < 1,
	});

export const catalogDetailQuery = (
	kind: CatalogKind,
	id: string,
	lang: string,
) =>
	queryOptions({
		queryKey: ["catalog", kind, lang, "detail", id],
		queryFn: ({ signal }) => api.catalogDetail(kind, id, lang, signal),
		staleTime: 10 * MINUTE,
	});
