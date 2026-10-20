import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { SearchXIcon } from "lucide-react";
import * as React from "react";
import { z } from "zod";
import {
	CatalogFilters,
	type Facet,
} from "@/components/catalog/catalog-filters";
import { CatalogGrid } from "@/components/catalog/catalog-grid";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import {
	Select,
	SelectItem,
	SelectPopup,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import type { CatalogKind } from "@/lib/api";
import {
	catalogListQuery,
	KIND_LABELS,
	type ListItem,
	loaderLanguage,
	searchText,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

const SORTS = ["default", "name", "rarity", "id"] as const;
type Sort = (typeof SORTS)[number];

const SORT_LABELS: Record<Sort, string> = {
	default: "Default order",
	name: "Name",
	rarity: "Rarity",
	id: "ID",
};

const searchSchema = z.object({
	q: searchText,
	/** Comma-separated `kind:code` tag filters (OR within a kind, AND across). */
	tags: searchText,
	/** Comma-separated rarities. */
	rarity: searchText,
	sort: z.enum(SORTS).optional().catch(undefined),
});

export const Route = createFileRoute("/catalog/$kind/")({
	validateSearch: searchSchema,
	loader: async ({ context, params }) => {
		const kind = params.kind as CatalogKind;
		// The items list is large; let the client fetch it instead of inlining
		// it into the SSR payload.
		if (typeof window === "undefined" && kind === "items") return;
		await context.queryClient
			.ensureQueryData(catalogListQuery(kind, loaderLanguage()))
			.catch(() => undefined);
	},
	head: ({ params }) => ({
		meta: [
			{
				title: pageTitle(KIND_LABELS[params.kind as CatalogKind] ?? "Catalog"),
			},
		],
	}),
	component: CatalogListPage,
});

const getScroller = () => document.getElementById("content");

function splitList(value: string | undefined): string[] {
	return value ? value.split(",").filter(Boolean) : [];
}

function joinList(values: Iterable<string | number>): string | undefined {
	const out = [...values].map(String);
	return out.length ? out.join(",") : undefined;
}

function CatalogListPage(): React.ReactElement {
	const { kind: rawKind } = Route.useParams();
	const kind = rawKind as CatalogKind;
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const query = useQuery({
		...catalogListQuery(kind, textLanguage),
		placeholderData: (prev, prevQuery) =>
			prevQuery?.queryKey[1] === kind ? prev : undefined,
	});
	const label = KIND_LABELS[kind];
	const all = query.data?.items;

	const selectedTags = React.useMemo(
		() => new Set(splitList(search.tags)),
		[search.tags],
	);
	const selectedRarities = React.useMemo(
		() => new Set(splitList(search.rarity).map(Number)),
		[search.rarity],
	);
	const sort: Sort = search.sort ?? "default";

	const facets = React.useMemo<Facet[]>(() => {
		if (!all) return [];
		const byKind = new Map<
			string,
			Map<string, { name: string; count: number }>
		>();
		for (const item of all) {
			for (const tag of item.tags) {
				let values = byKind.get(tag.kind);
				if (!values) {
					values = new Map();
					byKind.set(tag.kind, values);
				}
				const entry = values.get(tag.code);
				if (entry) entry.count++;
				else values.set(tag.code, { name: tag.name || tag.code, count: 1 });
			}
		}
		return [...byKind].map(([tagKind, values]) => ({
			kind: tagKind,
			values: [...values]
				.map(([code, v]) => ({ code, name: v.name, count: v.count }))
				.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
		}));
	}, [all]);

	const rarities = React.useMemo(() => {
		if (!all) return [];
		const set = new Set<number>();
		for (const item of all) if (item.rarity) set.add(item.rarity);
		return [...set].sort((a, b) => b - a);
	}, [all]);

	const deferredQ = React.useDeferredValue(search.q ?? "");
	const items = React.useMemo<ListItem[]>(() => {
		if (!all) return [];
		const tokens = deferredQ.trim().toLowerCase().split(/\s+/).filter(Boolean);
		const tagGroups = new Map<string, Set<string>>();
		for (const key of selectedTags) {
			const i = key.indexOf(":");
			const tagKind = key.slice(0, i);
			const set = tagGroups.get(tagKind) ?? new Set<string>();
			set.add(key.slice(i + 1));
			tagGroups.set(tagKind, set);
		}
		const out = all.filter((item) => {
			if (selectedRarities.size && !selectedRarities.has(item.rarity ?? 0))
				return false;
			for (const [tagKind, codes] of tagGroups) {
				if (!item.tags.some((t) => t.kind === tagKind && codes.has(t.code)))
					return false;
			}
			if (tokens.length) {
				const hay = `${item.name} ${item.id}`.toLowerCase();
				if (!tokens.every((t) => hay.includes(t))) return false;
			}
			return true;
		});
		if (sort === "name") out.sort((a, b) => a.name.localeCompare(b.name));
		else if (sort === "id") out.sort((a, b) => a.id.localeCompare(b.id));
		else if (sort === "rarity")
			out.sort(
				(a, b) =>
					(b.rarity ?? 0) - (a.rarity ?? 0) || a.name.localeCompare(b.name),
			);
		return out;
	}, [all, deferredQ, selectedTags, selectedRarities, sort]);

	const setSearch = (patch: Partial<z.infer<typeof searchSchema>>) =>
		void navigate({
			search: (prev) => ({ ...prev, ...patch }),
			replace: true,
			resetScroll: false,
		});

	const toggleTag = (key: string) => {
		const next = new Set(selectedTags);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		setSearch({ tags: joinList(next) });
	};
	const toggleRarity = (rarity: number) => {
		const next = new Set(selectedRarities);
		if (next.has(rarity)) next.delete(rarity);
		else next.add(rarity);
		setSearch({
			rarity: joinList([...next].sort((a, b) => b - a)),
		});
	};
	const filtered = Boolean(search.q || search.tags || search.rarity);
	const clear = () =>
		setSearch({ q: undefined, tags: undefined, rarity: undefined });

	const sortItems = SORTS.filter(
		(s) => s !== "rarity" || rarities.length > 1,
	).map((s) => ({ value: s, label: SORT_LABELS[s] }));

	return (
		<div className="mx-auto flex w-full max-w-7xl flex-col gap-5 p-4 md:p-6 lg:p-8">
			<PageHeader
				description={
					all
						? filtered
							? `${items.length.toLocaleString()} of ${all.length.toLocaleString()} ${label.toLowerCase()}`
							: `${all.length.toLocaleString()} ${label.toLowerCase()} · text in ${query.data?.lang ?? textLanguage}`
						: "Mapped from the game tables with resolved names."
				}
				title={label}
			/>

			{query.isError && !all ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : (
				<>
					<div className="flex flex-col gap-3">
						<div className="flex flex-wrap items-center gap-2">
							<SearchInput
								className="min-w-0 flex-1 sm:max-w-sm"
								label={`Search ${label}`}
								onCommit={(value) => setSearch({ q: value || undefined })}
								placeholder={`Search ${label.toLowerCase()} by name or id…`}
								value={search.q ?? ""}
							/>
							<Select
								items={sortItems}
								onValueChange={(value) =>
									setSearch({
										sort: value === "default" ? undefined : (value as Sort),
									})
								}
								value={sort}
							>
								<SelectTrigger
									aria-label="Sort by"
									className="w-auto min-w-36"
									size="sm"
								>
									<SelectValue />
								</SelectTrigger>
								<SelectPopup>
									{sortItems.map((item) => (
										<SelectItem key={item.value} value={item.value}>
											{item.label}
										</SelectItem>
									))}
								</SelectPopup>
							</Select>
						</div>
						{all ? (
							<CatalogFilters
								facets={facets}
								onClear={search.tags || search.rarity ? clear : undefined}
								onToggleRarity={toggleRarity}
								onToggleTag={toggleTag}
								rarities={rarities}
								selected={selectedTags}
								selectedRarities={selectedRarities}
							/>
						) : (
							<div className="flex flex-wrap gap-1.5">
								{Array.from({ length: 7 }, (_, i) => (
									// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
									<Skeleton className="h-7 w-20 rounded-full" key={i} />
								))}
							</div>
						)}
					</div>

					{!all ? (
						<div
							aria-busy="true"
							className="grid grid-cols-[repeat(auto-fill,minmax(9.75rem,1fr))] gap-3"
						>
							{Array.from({ length: 18 }, (_, i) => (
								// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
								<Skeleton className="aspect-[4/5] rounded-xl" key={i} />
							))}
						</div>
					) : items.length === 0 ? (
						<Empty className="rounded-2xl border border-dashed py-16">
							<EmptyHeader>
								<EmptyMedia variant="icon">
									<SearchXIcon />
								</EmptyMedia>
								<EmptyTitle>No {label.toLowerCase()} match</EmptyTitle>
								<EmptyDescription>
									{search.q
										? `Nothing matches “${search.q}” with the current filters.`
										: "No entries match the current filters."}
								</EmptyDescription>
							</EmptyHeader>
							<EmptyContent>
								<Button onClick={clear} variant="outline">
									Clear search and filters
								</Button>
							</EmptyContent>
						</Empty>
					) : (
						<CatalogGrid
							items={items}
							key={kind}
							kind={kind}
							label={label}
							scrollElement={getScroller}
						/>
					)}
				</>
			)}
		</div>
	);
}
