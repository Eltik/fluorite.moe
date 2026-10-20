import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { ConstructionIcon } from "lucide-react";
import * as React from "react";
import { z } from "zod";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import {
	CATALOG_KINDS,
	type CatalogItem,
	type CatalogKind,
	isApiError,
	thumbUrl,
} from "@/lib/api";
import { catalogQuery } from "@/lib/queries";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

const LABELS: Record<CatalogKind, string> = {
	operators: "Operators",
	enemies: "Enemies",
	weapons: "Weapons",
	items: "Items",
	npcs: "NPCs",
	facilities: "Facilities",
};

const searchSchema = z.object({
	q: z.string().optional().catch(undefined),
});

function isKind(value: string): value is CatalogKind {
	return (CATALOG_KINDS as readonly string[]).includes(value);
}

export const Route = createFileRoute("/catalog/$kind")({
	validateSearch: searchSchema,
	beforeLoad: ({ params }) => {
		if (!isKind(params.kind)) throw notFound();
	},
	head: ({ params }) => ({
		meta: [
			{
				title: pageTitle(isKind(params.kind) ? LABELS[params.kind] : "Catalog"),
			},
		],
	}),
	staticData: {
		section: "Catalog",
		crumb: ({ kind }) => (kind && isKind(kind) ? LABELS[kind] : "Catalog"),
	},
	component: CatalogPage,
});

function CatalogPage(): React.ReactElement {
	const { kind } = Route.useParams();
	const { q } = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const catalogKind = kind as CatalogKind;
	const query = useQuery(catalogQuery(catalogKind, textLanguage));
	const deferredQ = React.useDeferredValue(q ?? "");

	const items = React.useMemo(() => {
		const needle = deferredQ.trim().toLowerCase();
		const all = query.data?.items ?? [];
		if (!needle) return all;
		return all.filter(
			(item) =>
				item.name.toLowerCase().includes(needle) ||
				item.id.toLowerCase().includes(needle),
		);
	}, [query.data, deferredQ]);

	const notImplemented =
		isApiError(query.error) && query.error.isNotImplemented;

	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-6 p-4 md:p-6 lg:p-8">
			<PageHeader
				actions={
					!query.isError && (
						<SearchInput
							className="w-64 max-w-full"
							label={`Filter ${LABELS[catalogKind]}`}
							onCommit={(value) =>
								void navigate({
									search: { q: value || undefined },
									replace: true,
								})
							}
							placeholder={`Filter ${LABELS[catalogKind].toLowerCase()}…`}
							value={q ?? ""}
						/>
					)
				}
				description={
					query.data
						? `${query.data.items.length.toLocaleString()} entries · text in ${textLanguage}`
						: "Mapped from the game tables with resolved names."
				}
				title={LABELS[catalogKind]}
			/>

			{notImplemented ? (
				<Empty className="rounded-2xl border border-dashed">
					<EmptyHeader>
						<EmptyMedia variant="icon">
							<ConstructionIcon />
						</EmptyMedia>
						<EmptyTitle>{LABELS[catalogKind]} are not mapped yet</EmptyTitle>
						<EmptyDescription>
							The raw data is already browsable under Data › Tables. This
							catalog appears here once the API maps it.
						</EmptyDescription>
					</EmptyHeader>
				</Empty>
			) : query.isError ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : query.isPending ? (
				<div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-3">
					{Array.from({ length: 18 }, (_, i) => (
						// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
						<Skeleton className="aspect-[4/5] rounded-xl" key={i} />
					))}
				</div>
			) : items.length === 0 ? (
				<Empty>
					<EmptyHeader>
						<EmptyTitle>No matches</EmptyTitle>
						<EmptyDescription>Nothing matches “{q}”.</EmptyDescription>
					</EmptyHeader>
				</Empty>
			) : (
				<ul className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-3">
					{items.map((item) => (
						<li key={item.id}>
							<CatalogTile item={item} />
						</li>
					))}
				</ul>
			)}
		</div>
	);
}

function CatalogTile({ item }: { item: CatalogItem }): React.ReactElement {
	return (
		<Card className="overflow-hidden">
			<div className="flex aspect-square items-center justify-center bg-checker">
				{item.icon ? (
					<img
						alt=""
						className="size-full object-contain"
						decoding="async"
						loading="lazy"
						src={thumbUrl(item.icon, 256)}
					/>
				) : (
					<span className="font-semibold text-2xl text-muted-foreground">
						{item.name.slice(0, 2)}
					</span>
				)}
			</div>
			<div className="flex flex-col gap-1 p-2.5">
				<span className="truncate font-medium text-sm" title={item.name}>
					{item.name}
				</span>
				<div className="flex items-center gap-1.5">
					{item.rarity !== undefined && (
						<Badge size="sm" variant="outline">
							{"★".repeat(item.rarity)}
						</Badge>
					)}
					<span className="truncate font-mono text-muted-foreground text-xs">
						{item.id}
					</span>
				</div>
			</div>
		</Card>
	);
}
