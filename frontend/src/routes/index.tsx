import { useQuery } from "@tanstack/react-query";
import {
	createFileRoute,
	Link,
	type LinkOptions,
} from "@tanstack/react-router";
import {
	ArrowUpRightIcon,
	BoxesIcon,
	ClockIcon,
	FileIcon,
	HistoryIcon,
	SearchIcon,
	TagIcon,
	XIcon,
} from "lucide-react";
import type * as React from "react";
import { PageHeader } from "@/components/page-header";
import { useCommandPalette } from "@/components/shell/command-palette";
import { ErrorState } from "@/components/status-views";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Kbd } from "@/components/ui/kbd";
import { Skeleton } from "@/components/ui/skeleton";
import type { Root, StatusCounts } from "@/lib/api";
import {
	basename,
	dirname,
	formatDateTime,
	formatNumber,
	formatRelative,
} from "@/lib/format";
import { fileTarget, ROOT_META } from "@/lib/nav";
import { statusQuery } from "@/lib/queries";
import { clearRecent, useRecent } from "@/lib/recent";
import { pageTitle } from "@/lib/route-meta";

export const Route = createFileRoute("/")({
	loader: async ({ context }) => {
		// Non-blocking for failures: the page renders an offline state instead.
		await context.queryClient.query(statusQuery()).catch(() => undefined);
	},
	head: () => ({ meta: [{ title: pageTitle("Overview") }] }),
	staticData: { crumb: "Overview" },
	component: Home,
});

const ROOT_LINKS: {
	root: Root;
	count: keyof StatusCounts;
	link: LinkOptions;
}[] = [
	{ root: "tables", count: "tables", link: { to: "/tables" } },
	{ root: "textures", count: "textures", link: { to: "/textures" } },
	{
		root: "meshes",
		count: "meshes",
		link: { to: "/browse/$root", params: { root: "meshes" } },
	},
	{
		root: "jsondata",
		count: "jsondata",
		link: { to: "/browse/$root", params: { root: "jsondata" } },
	},
	{
		root: "lua",
		count: "lua",
		link: { to: "/browse/$root", params: { root: "lua" } },
	},
	{
		root: "gamedata",
		count: "gamedata",
		link: { to: "/browse/$root", params: { root: "gamedata" } },
	},
];

function Home(): React.ReactElement {
	const status = useQuery(statusQuery());
	const palette = useCommandPalette();

	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-8 p-4 md:p-6 lg:p-8">
			<PageHeader
				actions={
					<Button onClick={() => palette.setOpen(true)} variant="outline">
						<SearchIcon />
						Search everything
						<Kbd className="ms-1">⌘K</Kbd>
					</Button>
				}
				description="Browse the decoded tables, textures, models and scripts of Arknights: Endfield."
				title="Overview"
			/>

			{status.isError ? (
				<Card className="p-2">
					<ErrorState
						error={status.error}
						onRetry={() => void status.refetch()}
					/>
				</Card>
			) : (
				<section
					aria-labelledby="build-heading"
					className="flex flex-col gap-3"
				>
					<h2 className="sr-only" id="build-heading">
						Current build
					</h2>
					<div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
						<StatCard
							icon={<TagIcon />}
							label="Game version"
							loading={status.isPending}
							sub={status.data?.resVersion}
							value={status.data?.version}
						/>
						<StatCard
							icon={<ClockIcon />}
							label="Last update"
							loading={status.isPending}
							sub={
								status.data ? formatDateTime(status.data.updatedAt) : undefined
							}
							value={
								status.data ? formatRelative(status.data.updatedAt) : undefined
							}
						/>
						<StatCard
							icon={<FileIcon />}
							label="Indexed files"
							loading={status.isPending}
							sub="across all data roots"
							value={
								status.data
									? formatNumber(
											Object.values(status.data.counts).reduce(
												(a, b) => a + b,
												0,
											),
										)
									: undefined
							}
						/>
						<StatCard
							icon={<BoxesIcon />}
							label="Schema"
							loading={status.isPending}
							sub="API data schema"
							value={status.data ? `v${status.data.schemaVersion}` : undefined}
						/>
					</div>
				</section>
			)}

			<section
				aria-labelledby="explore-heading"
				className="flex flex-col gap-3"
			>
				<h2 className="font-medium text-sm" id="explore-heading">
					Explore
				</h2>
				<div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
					{ROOT_LINKS.map(({ root, count, link }) => {
						const meta = ROOT_META[root];
						const Icon = meta.icon;
						const value = status.data?.counts[count];
						return (
							<Card
								className="group transition-colors hover:bg-accent/40 has-focus-visible:ring-2 has-focus-visible:ring-ring"
								key={root}
							>
								<Link
									{...link}
									className="flex items-start gap-3 p-4 outline-none"
								>
									<div className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background text-muted-foreground [&_svg]:size-4.5">
										<Icon />
									</div>
									<div className="min-w-0 flex-1">
										<div className="flex items-center justify-between gap-2">
											<span className="font-medium text-sm">{meta.label}</span>
											<ArrowUpRightIcon className="size-4 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
										</div>
										<p className="truncate text-muted-foreground text-xs">
											{meta.description}
										</p>
										<div className="mt-2 font-mono text-muted-foreground text-xs tabular-nums">
											{value === undefined ? (
												status.isPending ? (
													<Skeleton className="h-4 w-16" />
												) : (
													"—"
												)
											) : (
												`${formatNumber(value)} files`
											)}
										</div>
									</div>
								</Link>
							</Card>
						);
					})}
				</div>
			</section>

			<RecentFiles />
		</div>
	);
}

function StatCard({
	icon,
	label,
	value,
	sub,
	loading,
}: {
	icon: React.ReactNode;
	label: string;
	value?: string;
	sub?: string;
	loading: boolean;
}): React.ReactElement {
	return (
		<Card className="gap-1 p-4">
			<div className="flex items-center gap-2 text-muted-foreground text-xs [&_svg]:size-3.5">
				{icon}
				{label}
			</div>
			{loading ? (
				<>
					<Skeleton className="mt-1 h-7 w-24" />
					<Skeleton className="h-4 w-36" />
				</>
			) : (
				<>
					<div className="truncate font-semibold text-2xl tabular-nums tracking-tight">
						{value ?? "—"}
					</div>
					<div
						className="truncate font-mono text-muted-foreground text-xs"
						title={sub}
					>
						{sub ?? " "}
					</div>
				</>
			)}
		</Card>
	);
}

function RecentFiles(): React.ReactElement {
	const recent = useRecent();
	return (
		<section aria-labelledby="recent-heading" className="flex flex-col gap-3">
			<div className="flex items-center justify-between">
				<h2 className="font-medium text-sm" id="recent-heading">
					Recently opened
				</h2>
				{recent.length > 0 && (
					<Button onClick={clearRecent} size="xs" variant="ghost">
						<XIcon />
						Clear
					</Button>
				)}
			</div>
			{recent.length === 0 ? (
				<Card className="flex-row items-center gap-3 p-4 text-muted-foreground text-sm">
					<HistoryIcon className="size-4 shrink-0" />
					Files you open in the explorers show up here, on this device only.
				</Card>
			) : (
				<Card className="overflow-hidden p-1">
					<ul className="divide-y">
						{recent.map((entry) => {
							const Icon = ROOT_META[entry.root].icon;
							return (
								<li key={`${entry.root}:${entry.path}`}>
									<Link
										{...fileTarget(entry.root, entry.path)}
										className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
									>
										<Icon className="size-4 shrink-0 text-muted-foreground" />
										<span className="min-w-0 flex-1 truncate">
											{basename(entry.path)}
											<span className="ms-2 text-muted-foreground text-xs">
												{ROOT_META[entry.root].label}
												{dirname(entry.path) && ` / ${dirname(entry.path)}`}
											</span>
										</span>
										<span className="shrink-0 text-muted-foreground text-xs">
											{formatRelative(new Date(entry.at).toISOString())}
										</span>
									</Link>
								</li>
							);
						})}
					</ul>
				</Card>
			)}
		</section>
	);
}
