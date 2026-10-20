import { useQuery } from "@tanstack/react-query";
import { Link, type LinkOptions } from "@tanstack/react-router";
import { XIcon } from "lucide-react";
import type * as React from "react";
import {
	clearRecentEntities,
	useRecentEntities,
} from "@/components/home/recent-entities";
import { NAV } from "@/components/shell/nav-config";
import { RelativeTime } from "@/components/shell/relative-time";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import type { StatusCounts } from "@/lib/api";
import { KIND_SINGULAR } from "@/lib/catalog";
import { basename, dirname, formatCompact, formatNumber } from "@/lib/format";
import { fileTarget, ROOT_META } from "@/lib/nav";
import { catalogKindsQuery, statusQuery } from "@/lib/queries";
import { clearRecent, useRecent } from "@/lib/recent";

const CATALOG = NAV.find((s) => s.id === "catalog")?.items ?? [];
const FILES = NAV.find((s) => s.id === "files")?.items ?? [];

/** `/textures` → `textures` count, `/browse/meshes` → `meshes`. */
function countKey(href: string): keyof StatusCounts | undefined {
	const key = href.split("/").pop();
	return key && key in ROOT_META ? (key as keyof StatusCounts) : undefined;
}

function SectionHeading({
	id,
	title,
	description,
	action,
}: {
	id: string;
	title: string;
	description?: string;
	action?: React.ReactNode;
}): React.ReactElement {
	return (
		<div className="flex items-end justify-between gap-4">
			<div>
				<h2 className="font-semibold text-lg tracking-tight" id={id}>
					{title}
				</h2>
				{description && (
					<p className="text-muted-foreground text-sm">{description}</p>
				)}
			</div>
			{action}
		</div>
	);
}

export function CatalogLinks(): React.ReactElement {
	const counts = useQuery(catalogKindsQuery());
	return (
		<section aria-labelledby="catalog-heading" className="flex flex-col gap-3">
			<SectionHeading
				description="Stats, skills, drops and lore for everything in the game."
				id="catalog-heading"
				title="Catalog"
			/>
			<ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
				{CATALOG.map((item) => {
					const Icon = item.icon;
					const count = item.catalogKind
						? counts.data?.[item.catalogKind]
						: undefined;
					return (
						<li key={item.href}>
							<Link
								{...item.link}
								className="group flex h-full flex-col gap-3 rounded-2xl border bg-card p-4 transition-colors hover:bg-accent"
							>
								<Icon
									aria-hidden="true"
									className="size-5 text-muted-foreground transition-colors group-hover:text-brand"
								/>
								<span>
									<span className="block font-semibold text-2xl tabular-nums tracking-tight">
										{count !== undefined ? (
											formatNumber(count)
										) : counts.isPending ? (
											<Skeleton className="h-8 w-12" />
										) : (
											"—"
										)}
									</span>
									<span className="text-muted-foreground text-sm">
										{item.label}
									</span>
								</span>
							</Link>
						</li>
					);
				})}
			</ul>
		</section>
	);
}

interface RecentRow {
	key: string;
	at: number;
	label: string;
	meta: string;
	link: LinkOptions;
}

/** Catalog pages and files opened on this device; client-only (localStorage). */
export function RecentlyViewed(): React.ReactElement | null {
	const files = useRecent();
	const entities = useRecentEntities();
	if (files.length === 0 && entities.length === 0) return null;

	const rows: RecentRow[] = [
		...entities.map((e) => ({
			key: `e:${e.kind}:${e.id}`,
			at: e.at,
			label: e.name,
			meta: KIND_SINGULAR[e.kind],
			link: {
				to: "/catalog/$kind/$id",
				params: { kind: e.kind, id: e.id },
			} satisfies LinkOptions as LinkOptions,
		})),
		...files.map((f) => ({
			key: `f:${f.root}:${f.path}`,
			at: f.at,
			label: basename(f.path),
			meta: [ROOT_META[f.root].label, dirname(f.path)]
				.filter(Boolean)
				.join(" / "),
			link: fileTarget(f.root, f.path) as LinkOptions,
		})),
	]
		.sort((a, b) => b.at - a.at)
		.slice(0, 8);

	return (
		<section aria-labelledby="recent-heading" className="flex flex-col gap-3">
			<SectionHeading
				action={
					<Button
						onClick={() => {
							clearRecent();
							clearRecentEntities();
						}}
						size="xs"
						variant="ghost"
					>
						<XIcon />
						Clear
					</Button>
				}
				description="On this device only."
				id="recent-heading"
				title="Recently viewed"
			/>
			<Card className="overflow-hidden p-1">
				<ul className="divide-y">
					{rows.map((row) => (
						<li key={row.key}>
							<Link
								{...row.link}
								className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm hover:bg-accent"
							>
								<span className="min-w-0 flex-1 truncate">
									<span className="font-medium">{row.label}</span>
									<span className="ms-2 text-muted-foreground text-xs">
										{row.meta}
									</span>
								</span>
								<RelativeTime
									className="shrink-0 text-muted-foreground text-xs"
									iso={new Date(row.at).toISOString()}
								/>
							</Link>
						</li>
					))}
				</ul>
			</Card>
		</section>
	);
}

/** Demoted raw-data roots plus the build line (version, freshness, size). */
export function GameFiles(): React.ReactElement {
	const status = useQuery(statusQuery());
	const total = status.data
		? Object.values(status.data.counts).reduce((a, b) => a + b, 0)
		: undefined;

	return (
		<section aria-labelledby="files-heading" className="flex flex-col gap-3">
			<SectionHeading
				description="The decoded data behind everything above, for dataminers and modders."
				id="files-heading"
				title="Game files"
			/>
			<ul className="grid gap-x-6 gap-y-1 sm:grid-cols-2 lg:grid-cols-3">
				{FILES.map((item) => {
					const Icon = item.icon;
					const key = countKey(item.href);
					const count = key ? status.data?.counts[key] : undefined;
					const description = key ? ROOT_META[key].description : undefined;
					return (
						<li key={item.href}>
							<Link
								{...item.link}
								className="flex items-center gap-3 rounded-lg px-2 py-2 text-sm hover:bg-accent"
							>
								<Icon
									aria-hidden="true"
									className="size-4 shrink-0 text-muted-foreground"
								/>
								<span className="min-w-0 flex-1">
									<span className="font-medium">{item.label}</span>
									{description && (
										<span className="block truncate text-muted-foreground text-xs">
											{description}
										</span>
									)}
								</span>
								{count !== undefined && (
									<span
										className="shrink-0 text-muted-foreground text-xs tabular-nums"
										title={`${formatNumber(count)} files`}
									>
										{formatCompact(count)}
									</span>
								)}
							</Link>
						</li>
					);
				})}
			</ul>
			<p className="flex flex-wrap items-center gap-x-2 gap-y-1 border-t pt-3 text-muted-foreground text-xs">
				{status.data ? (
					<>
						<span>
							Game build{" "}
							<span className="font-mono text-foreground">
								{status.data.version}
							</span>
						</span>
						<span aria-hidden="true">·</span>
						<span>
							updated <RelativeTime iso={status.data.updatedAt} />
						</span>
						{total !== undefined && (
							<>
								<span aria-hidden="true">·</span>
								<span title={`${formatNumber(total)} files`}>
									{formatCompact(total)} files indexed
								</span>
							</>
						)}
					</>
				) : status.isError ? (
					"The data API is offline; counts will appear once it is back."
				) : (
					<Skeleton className="h-4 w-64" />
				)}
			</p>
		</section>
	);
}
