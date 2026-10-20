import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { ClockIcon, SearchXIcon, TableIcon } from "lucide-react";
import * as React from "react";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState, ListSkeleton } from "@/components/status-views";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { TableInfo } from "@/lib/api";
import { formatBytes, formatNumber } from "@/lib/format";
import { tablesQuery } from "@/lib/queries";
import { useRecent } from "@/lib/recent";
import { pageTitle } from "@/lib/route-meta";
import { oneOf, searchParams, text } from "@/lib/search-params";

const parseSearch = searchParams({
	q: text,
	sort: oneOf(["name", "size"]),
});

export const Route = createFileRoute("/tables/")({
	validateSearch: parseSearch,
	loader: ({ context }) => context.queryClient.query(tablesQuery()),
	head: () => ({ meta: [{ title: pageTitle("Tables") }] }),
	pendingComponent: () => <ListSkeleton className="p-6" rows={20} />,
	component: TablesPage,
});

/** Leading CamelCase word, used to group related tables (Char*, Item*, …). */
function groupOf(name: string): string {
	const match = name.match(/^[A-Z][a-z0-9]*/);
	return match ? match[0] : name.slice(0, 1).toUpperCase();
}

function TablesPage(): React.ReactElement {
	const { q, sort = "name" } = Route.useSearch();
	const navigate = Route.useNavigate();
	const query = useQuery(tablesQuery());
	const needle = (q ?? "").trim().toLowerCase();

	const groups = React.useMemo(() => {
		const tables = (query.data?.tables ?? []).filter(
			(t) => !needle || t.name.toLowerCase().includes(needle),
		);
		if (sort === "size") {
			return [
				{
					name: "By size",
					tables: [...tables].sort((a, b) => b.size - a.size),
				},
			];
		}
		const map = new Map<string, TableInfo[]>();
		for (const t of [...tables].sort((a, b) => a.name.localeCompare(b.name))) {
			const g = groupOf(t.name);
			map.set(g, [...(map.get(g) ?? []), t]);
		}
		// Fold tiny groups into "Other" to keep the page scannable.
		const out: { name: string; tables: TableInfo[] }[] = [];
		const other: TableInfo[] = [];
		for (const [name, list] of map) {
			if (list.length < 3) other.push(...list);
			else out.push({ name, tables: list });
		}
		if (other.length) out.push({ name: "Other", tables: other });
		return out;
	}, [query.data, needle, sort]);

	const shown = groups.reduce((n, g) => n + g.tables.length, 0);

	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-6 p-4 md:p-6 lg:p-8">
			<PageHeader
				actions={
					<>
						<span
							className="text-muted-foreground text-xs max-sm:hidden"
							id="tables-sort-label"
						>
							Sort by
						</span>
						<ToggleGroup
							aria-labelledby="tables-sort-label"
							onValueChange={(value) => {
								const next = value[0];
								if (next === "name" || next === "size") {
									void navigate({
										search: (prev) => ({
											...prev,
											sort: next === "name" ? undefined : next,
										}),
										replace: true,
									});
								}
							}}
							size="sm"
							value={[sort]}
							variant="outline"
						>
							<ToggleGroupItem
								aria-label="Name, grouped by prefix"
								title="Name, grouped by prefix"
								value="name"
							>
								Name A–Z
							</ToggleGroupItem>
							<ToggleGroupItem
								aria-label="Size, largest first"
								title="Size, largest first"
								value="size"
							>
								Size ↓
							</ToggleGroupItem>
						</ToggleGroup>
						<SearchInput
							className="w-64 max-w-full"
							label="Filter tables"
							onCommit={(value) =>
								void navigate({
									search: (prev) => ({ ...prev, q: value || undefined }),
									replace: true,
								})
							}
							placeholder="Filter tables…"
							value={q ?? ""}
						/>
					</>
				}
				description={
					query.data
						? needle
							? `${formatNumber(shown)} of ${formatNumber(query.data.tables.length)} tables`
							: `${formatNumber(query.data.tables.length)} decoded game tables`
						: undefined
				}
				icon={<TableIcon />}
				title="Tables"
			/>

			{!needle && <RecentTables />}

			{query.isError ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : shown === 0 ? (
				<Empty>
					<EmptyHeader>
						<EmptyMedia variant="icon">
							<SearchXIcon />
						</EmptyMedia>
						<EmptyTitle>No tables match “{q}”</EmptyTitle>
						<EmptyDescription>
							Try a shorter name, like “Item” or “Char”.
						</EmptyDescription>
					</EmptyHeader>
				</Empty>
			) : (
				<div className="flex flex-col gap-6">
					{groups.map((group) => (
						<section aria-label={group.name} key={group.name}>
							<h2 className="mb-2 flex items-baseline gap-2 font-medium text-sm">
								{group.name}
								<span className="font-normal text-muted-foreground text-xs tabular-nums">
									{group.tables.length}
								</span>
							</h2>
							<ul className="grid grid-cols-1 gap-x-4 sm:grid-cols-2 lg:grid-cols-3">
								{group.tables.map((t) => (
									<li key={t.name}>
										<Link
											className="flex h-10 items-center justify-between gap-3 rounded-md px-2 text-sm sm:h-8 outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
											params={{ name: t.name }}
											to="/tables/$name"
										>
											<span className="truncate" title={t.name}>
												<Highlight needle={needle} text={t.name} />
											</span>
											<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
												{formatBytes(t.size)}
											</span>
										</Link>
									</li>
								))}
							</ul>
						</section>
					))}
				</div>
			)}
		</div>
	);
}

/** Tables opened recently in this browser (from the shared recent list). */
function RecentTables(): React.ReactElement | null {
	const recent = useRecent()
		.filter((e) => e.root === "tables" && e.path.endsWith(".json"))
		.slice(0, 6);
	if (recent.length === 0) return null;
	return (
		<section aria-label="Recently opened">
			<h2 className="mb-2 flex items-center gap-1.5 font-medium text-sm">
				<ClockIcon className="size-3.5 text-muted-foreground" />
				Recently opened
			</h2>
			<ul className="flex flex-wrap gap-1.5">
				{recent.map((e) => {
					const name = e.path.replace(/\.json$/, "");
					return (
						<li key={e.path}>
							<Link
								className="inline-flex h-7 items-center rounded-md border bg-card px-2.5 text-sm shadow-xs/5 outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
								params={{ name }}
								to="/tables/$name"
							>
								{name}
							</Link>
						</li>
					);
				})}
			</ul>
		</section>
	);
}

function Highlight({ text, needle }: { text: string; needle: string }) {
	if (!needle) return text;
	const i = text.toLowerCase().indexOf(needle);
	if (i === -1) return text;
	return (
		<>
			{text.slice(0, i)}
			<mark className="rounded-[2px] bg-amber-400/30 text-inherit">
				{text.slice(i, i + needle.length)}
			</mark>
			{text.slice(i + needle.length)}
		</>
	);
}
