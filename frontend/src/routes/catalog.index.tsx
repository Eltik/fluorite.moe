import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import {
	ArrowRightIcon,
	BookOpenIcon,
	FactoryIcon,
	type LucideIcon,
	PackageIcon,
	SkullIcon,
	SwordIcon,
	UserRoundIcon,
	UsersIcon,
} from "lucide-react";
import type * as React from "react";
import { PageHeader } from "@/components/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { type CatalogKind, getJson } from "@/lib/api";
import { KIND_LABELS, KIND_ORDER } from "@/lib/catalog";
import { formatNumber } from "@/lib/format";
import { pageTitle } from "@/lib/route-meta";

const KIND_ICONS: Record<CatalogKind, LucideIcon> = {
	operators: UsersIcon,
	weapons: SwordIcon,
	enemies: SkullIcon,
	items: PackageIcon,
	npcs: UserRoundIcon,
	facilities: FactoryIcon,
};

const KIND_BLURBS: Record<CatalogKind, string> = {
	operators: "Stats, skills, talents, voice lines and art",
	weapons: "Base ATK by level, skills and breakthroughs",
	enemies: "Abilities, resistances and level scaling",
	items: "Materials, gear and how to obtain them",
	npcs: "Named characters and their models",
	facilities: "AIC buildings, power and footprint",
};

const indexQuery = {
	queryKey: ["catalog", "index"],
	queryFn: ({ signal }: { signal: AbortSignal }) =>
		getJson<{ kinds: { kind: CatalogKind; count: number }[] }>(
			"/api/catalog",
			undefined,
			signal,
		),
	staleTime: 10 * 60_000,
};

export const Route = createFileRoute("/catalog/")({
	loader: ({ context }) =>
		context.queryClient.ensureQueryData(indexQuery).catch(() => undefined),
	head: () => ({ meta: [{ title: pageTitle("Catalog") }] }),
	staticData: { section: "Catalog", crumb: "Overview" },
	component: CatalogIndex,
});

function CatalogIndex(): React.ReactElement {
	const { data } = useQuery(indexQuery);
	const counts = new Map(data?.kinds.map((k) => [k.kind, k.count]));
	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-6 p-4 md:p-6 lg:p-8">
			<PageHeader
				description="Game entities mapped from the tables, with names in your text language."
				title="Catalog"
			/>
			<ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
				{KIND_ORDER.map((kind) => {
					const Icon = KIND_ICONS[kind];
					const count = counts.get(kind);
					return (
						<li key={kind}>
							<Link
								className="flex h-full items-start gap-3 rounded-xl border bg-card p-4 shadow-xs/5 outline-none transition-[box-shadow,border-color] hover:border-foreground/16 hover:shadow-md/5 focus-visible:ring-2 focus-visible:ring-ring"
								params={{ kind }}
								to="/catalog/$kind"
							>
								<span className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted text-muted-foreground">
									<Icon className="size-4.5" />
								</span>
								<span className="flex min-w-0 flex-col gap-0.5">
									<span className="flex items-baseline gap-2 font-medium">
										{KIND_LABELS[kind]}
										{count !== undefined ? (
											<span className="text-muted-foreground text-xs tabular-nums">
												{formatNumber(count)}
											</span>
										) : (
											<Skeleton className="h-3 w-8" />
										)}
									</span>
									<span className="text-muted-foreground text-sm">
										{KIND_BLURBS[kind]}
									</span>
								</span>
							</Link>
						</li>
					);
				})}
				<li className="sm:col-span-2 lg:col-span-3">
					<Link
						className="group flex h-full flex-col gap-3 rounded-xl border bg-card p-4 shadow-xs/5 outline-none transition-[box-shadow,border-color] hover:border-foreground/16 hover:shadow-md/5 focus-visible:ring-2 focus-visible:ring-ring sm:flex-row sm:items-center sm:gap-4"
						to="/story"
					>
						<span className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted text-muted-foreground">
							<BookOpenIcon className="size-4.5" />
						</span>
						<span className="flex min-w-0 flex-1 flex-col gap-0.5">
							<span className="font-medium">Story</span>
							<span className="text-muted-foreground text-sm">
								Every mission, level and NPC dialog, read as a chat transcript
								with choices and both Endministrator variants.
							</span>
						</span>
						<span className="inline-flex shrink-0 items-center gap-1 font-medium text-muted-foreground text-sm transition-colors group-hover:text-foreground">
							Start reading
							<ArrowRightIcon className="size-4" />
						</span>
					</Link>
				</li>
			</ul>
		</div>
	);
}
