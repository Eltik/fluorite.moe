import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import {
	ArrowDownWideNarrowIcon,
	ArrowUpNarrowWideIcon,
	SearchXIcon,
} from "lucide-react";
import * as React from "react";
import { z } from "zod";
import {
	CatalogFilters,
	type Facet,
} from "@/components/catalog/catalog-filters";
import { CatalogGrid, type GridItem } from "@/components/catalog/catalog-grid";
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
	facilityTint,
	humanizeCode,
	itemTypeRank,
	KIND_LABELS,
	type ListItem,
	loaderLanguage,
	searchText,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

const SORTS = ["default", "name", "rarity", "id"] as const;
type Sort = (typeof SORTS)[number];
type Dir = "asc" | "desc";

function sortLabel(sort: Sort, kind: CatalogKind): string {
	switch (sort) {
		case "default":
			return kind === "items" ? "Type, then rarity" : "Game order";
		case "name":
			return "Name";
		case "rarity":
			return "Rarity";
		case "id":
			return "ID";
	}
}

/** The direction each sort reads naturally in. */
const NATURAL_DIR: Record<Sort, Dir> = {
	default: "asc",
	name: "asc",
	rarity: "desc",
	id: "asc",
};

const searchSchema = z.object({
	q: searchText,
	/** Comma-separated `kind:code` tag filters (OR within a kind, AND across). */
	tags: searchText,
	/** Comma-separated rarities. */
	rarity: searchText,
	sort: z.enum(SORTS).optional().catch(undefined),
	dir: z.enum(["asc", "desc"]).optional().catch(undefined),
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

function dirLabel(sort: Sort, dir: Dir): string {
	if (sort === "name" || sort === "id") return dir === "asc" ? "A–Z" : "Z–A";
	if (sort === "rarity")
		return dir === "desc" ? "Highest first" : "Lowest first";
	return dir === "asc" ? "Default" : "Reversed";
}

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
	// "NPCs" stays an acronym in running text.
	const lower = kind === "npcs" ? label : label.toLowerCase();
	const raw = query.data?.items;
	// Facilities without a localized name fall back to their id, and their
	// types are raw codes; make both readable and tint by category.
	const all = React.useMemo<GridItem[] | undefined>(() => {
		if (!raw || kind !== "facilities") return raw;
		return raw.map((item) => {
			const type = item.tags.find((t) => t.kind === "buildingType");
			return {
				...item,
				name: item.name === item.id ? humanizeCode(item.id) : item.name,
				tags: item.tags.map((t) =>
					t.kind === "buildingType" && t.name === t.code
						? { ...t, name: humanizeCode(t.code) }
						: t,
				),
				tint: facilityTint(type?.code),
			};
		});
	}, [raw, kind]);

	const selectedTags = React.useMemo(
		() => new Set(splitList(search.tags)),
		[search.tags],
	);
	const selectedRarities = React.useMemo(
		() => new Set(splitList(search.rarity).map(Number)),
		[search.rarity],
	);
	const sort: Sort = search.sort ?? "default";

	const dir: Dir = search.dir ?? NATURAL_DIR[sort];

	const tagGroups = React.useMemo(() => {
		const groups = new Map<string, Set<string>>();
		for (const key of selectedTags) {
			const i = key.indexOf(":");
			const tagKind = key.slice(0, i);
			const set = groups.get(tagKind) ?? new Set<string>();
			// A chip can stand for several codes sharing one name (`a|b`).
			for (const code of key.slice(i + 1).split("|")) set.add(code);
			groups.set(tagKind, set);
		}
		return groups;
	}, [selectedTags]);

	const deferredQ = React.useDeferredValue(search.q ?? "");
	const tokens = React.useMemo(
		() => deferredQ.trim().toLowerCase().split(/\s+/).filter(Boolean),
		[deferredQ],
	);

	/** Whether `item` passes every active filter except the one named `skip`. */
	const passes = React.useCallback(
		(item: ListItem, skip?: string) => {
			if (
				skip !== "rarity" &&
				selectedRarities.size &&
				!selectedRarities.has(item.rarity ?? 0)
			)
				return false;
			for (const [tagKind, codes] of tagGroups) {
				if (tagKind === skip) continue;
				if (!item.tags.some((t) => t.kind === tagKind && codes.has(t.code)))
					return false;
			}
			if (tokens.length) {
				const hay = `${item.name} ${item.id}`.toLowerCase();
				if (!tokens.every((t) => hay.includes(t))) return false;
			}
			return true;
		},
		[selectedRarities, tagGroups, tokens],
	);

	/** Item-type sizes, used to rank the types the order list does not name. */
	const typeCounts = React.useMemo(() => {
		const counts = new Map<string, number>();
		for (const t of query.data?.types ?? []) counts.set(t.code, t.count);
		return counts;
	}, [query.data?.types]);

	// Facet values keep a stable order (by overall size, or by browse order for
	// item types); counts reflect the other active filters.
	const facets = React.useMemo<Facet[]>(() => {
		if (!all) return [];
		const byKind = new Map<
			string,
			Map<string, { name: string; total: number; count: number }>
		>();
		for (const item of all) {
			for (const tag of item.tags) {
				let values = byKind.get(tag.kind);
				if (!values) {
					values = new Map();
					byKind.set(tag.kind, values);
				}
				const entry = values.get(tag.code);
				if (entry) entry.total++;
				else
					values.set(tag.code, {
						name: tag.name || tag.code,
						total: 1,
						count: 0,
					});
			}
		}
		for (const [tagKind, values] of byKind) {
			for (const item of all) {
				if (!passes(item, tagKind)) continue;
				for (const tag of item.tags) {
					if (tag.kind !== tagKind) continue;
					const entry = values.get(tag.code);
					if (entry) entry.count++;
				}
			}
		}
		return [...byKind].map(([tagKind, values]) => {
			// Different codes can resolve to one name (NPC factions and titles are
			// per-character text keys); merge them into one chip.
			const byName = new Map<
				string,
				{ codes: string[]; name: string; count: number; total: number }
			>();
			for (const [code, v] of values) {
				const merged = byName.get(v.name);
				if (merged) {
					merged.codes.push(code);
					merged.count += v.count;
					merged.total += v.total;
				} else byName.set(v.name, { codes: [code], ...v });
			}
			return {
				kind: tagKind,
				values: [...byName.values()]
					.map((v) => ({
						code: v.codes.join("|"),
						name: v.name,
						count: v.count,
						total: v.total,
					}))
					.sort((a, b) =>
						tagKind === "itemType"
							? itemTypeRank(a.code.split("|")[0], typeCounts) -
									itemTypeRank(b.code.split("|")[0], typeCounts) ||
								a.name.localeCompare(b.name)
							: b.total - a.total || a.name.localeCompare(b.name),
					),
			};
		});
	}, [all, passes, typeCounts]);

	const rarities = React.useMemo(() => {
		if (!all) return [];
		const set = new Set<number>();
		for (const item of all) if (item.rarity) set.add(item.rarity);
		return [...set].sort((a, b) => b - a);
	}, [all]);

	const rarityCounts = React.useMemo(() => {
		const counts = new Map<number, number>();
		for (const item of all ?? []) {
			if (!item.rarity || !passes(item, "rarity")) continue;
			counts.set(item.rarity, (counts.get(item.rarity) ?? 0) + 1);
		}
		return counts;
	}, [all, passes]);

	const matched = React.useMemo<ListItem[]>(
		() => (all ? all.filter((item) => passes(item)) : []),
		[all, passes],
	);

	const items = React.useMemo<GridItem[]>(() => {
		let out: GridItem[] = matched;
		// Numbered series (`achv_…_1`, `_2`, `_3` with one name) collapse into
		// their first tier, unless the user is searching for a specific one.
		if (kind === "items" && tokens.length === 0) {
			const heads = new Map<string, GridItem>();
			const collapsed: GridItem[] = [];
			for (const item of matched) {
				const stem = item.id.replace(/_\d+$/, "");
				if (stem === item.id) {
					collapsed.push(item);
					continue;
				}
				const key = `${stem}\u0000${item.name}`;
				const head = heads.get(key);
				if (head) head.series = (head.series ?? 1) + 1;
				else {
					const copy: GridItem = { ...item, series: 1 };
					heads.set(key, copy);
					collapsed.push(copy);
				}
			}
			out = collapsed;
		} else {
			out = [...matched];
		}
		const byName = (a: ListItem, b: ListItem) => a.name.localeCompare(b.name);
		const byRarity = (a: ListItem, b: ListItem) =>
			(b.rarity ?? 0) - (a.rarity ?? 0);
		const typeOf = (item: ListItem) =>
			item.tags.find((t) => t.kind === "itemType")?.code;
		if (sort === "name") out.sort(byName);
		else if (sort === "id") out.sort((a, b) => a.id.localeCompare(b.id));
		else if (sort === "rarity")
			out.sort((a, b) => byRarity(a, b) || byName(a, b));
		else if (kind === "items")
			out.sort(
				(a, b) =>
					itemTypeRank(typeOf(a), typeCounts) -
						itemTypeRank(typeOf(b), typeCounts) ||
					byRarity(a, b) ||
					byName(a, b),
			);
		if (dir !== NATURAL_DIR[sort]) out.reverse();
		return out;
	}, [matched, kind, tokens.length, sort, dir, typeCounts]);

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
	const collapsedCount = matched.length - items.length;
	const clear = () =>
		setSearch({ q: undefined, tags: undefined, rarity: undefined });

	const sortItems = SORTS.filter(
		(s) => s !== "rarity" || rarities.length > 1,
	).map((s) => ({ value: s, label: sortLabel(s, kind) }));
	const reversed = dir !== NATURAL_DIR[sort];

	return (
		<div className="mx-auto flex w-full max-w-7xl flex-col gap-5 p-4 md:p-6 lg:p-8">
			<PageHeader
				description={
					all
						? filtered
							? `${matched.length.toLocaleString()} of ${all.length.toLocaleString()} ${lower}${collapsedCount > 0 ? ` · numbered series grouped into ${items.length.toLocaleString()} cards` : ""}`
							: `${all.length.toLocaleString()} ${lower} · text in ${query.data?.lang ?? textLanguage}${collapsedCount > 0 ? ` · numbered series grouped into ${items.length.toLocaleString()} cards` : ""}`
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
								placeholder={`Search ${lower} by name or id…`}
								value={search.q ?? ""}
							/>
							<Select
								items={sortItems}
								onValueChange={(value) =>
									setSearch({
										sort: value === "default" ? undefined : (value as Sort),
										dir: undefined,
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
							<Button
								aria-label={`Sort direction: ${dirLabel(sort, dir)}. Click to reverse.`}
								onClick={() =>
									setSearch({
										dir: reversed ? undefined : dir === "asc" ? "desc" : "asc",
									})
								}
								size={sort === "default" ? "icon-sm" : "sm"}
								title="Reverse the order"
								variant="outline"
							>
								{dir === "asc" ? (
									<ArrowUpNarrowWideIcon />
								) : (
									<ArrowDownWideNarrowIcon />
								)}
								{sort !== "default" && (
									<span className="max-sm:sr-only">{dirLabel(sort, dir)}</span>
								)}
							</Button>
						</div>
						{all ? (
							<CatalogFilters
								facets={facets}
								onClear={search.tags || search.rarity ? clear : undefined}
								onToggleRarity={toggleRarity}
								rarityCounts={rarityCounts}
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
								<EmptyTitle>No {lower} match</EmptyTitle>
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
