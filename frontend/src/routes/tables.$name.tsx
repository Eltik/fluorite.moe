import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import type { SortingState } from "@tanstack/react-table";
import {
	BracesIcon,
	CopyIcon,
	DownloadIcon,
	LanguagesIcon,
	Table2Icon,
} from "lucide-react";
import * as React from "react";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import { DataGrid } from "@/components/table/data-grid";
import { Button } from "@/components/ui/button";
import {
	Sheet,
	SheetDescription,
	SheetHeader,
	SheetPanel,
	SheetPopup,
	SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { Toggle } from "@/components/ui/toggle";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ViewerFallback } from "@/components/viewers/file-preview";
import { API_URL, api } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { formatNumber } from "@/lib/format";
import { tableQuery } from "@/lib/queries";
import { pushRecent } from "@/lib/recent";
import { pageTitle } from "@/lib/route-meta";
import { bool, nat, oneOf, searchParams, str } from "@/lib/search-params";
import { useSettings } from "@/lib/settings";
import { collectI18nIds, type Rec, toTabular } from "@/lib/table-shape";

const JsonViewer = React.lazy(() => import("@/components/viewers/json-viewer"));

const parseSearch = searchParams({
	view: oneOf(["grid", "json"]),
	q: str(),
	sort: str(),
	i18n: bool,
	row: nat,
});

function parseSort(value: string | undefined): SortingState {
	if (!value) return [];
	return value
		.split(",")
		.map((part) => {
			const i = part.lastIndexOf(":");
			const id = i === -1 ? part : part.slice(0, i);
			return { id, desc: part.slice(i + 1) === "desc" };
		})
		.filter((s) => s.id);
}

function formatSort(sorting: SortingState): string | undefined {
	if (sorting.length === 0) return undefined;
	return sorting.map((s) => `${s.id}:${s.desc ? "desc" : "asc"}`).join(",");
}

export const Route = createFileRoute("/tables/$name")({
	validateSearch: parseSearch,
	loader: ({ context, params, preload }) => {
		// Table bodies can be many MB: never SSR/dehydrate them, and don't
		// prefetch on hover; start the request early on real client navigations.
		if (typeof window !== "undefined" && !preload) {
			void context.queryClient.prefetchQuery(tableQuery(params.name));
		}
	},
	head: ({ params }) => ({
		meta: [{ title: pageTitle(params.name, "Tables") }],
	}),
	staticData: { crumb: ({ name }) => name ?? "" },
	component: TablePage,
});

const I18N_CHUNK = 4_000;

function useI18nMap(
	lang: string,
	name: string,
	ids: string[],
	enabled: boolean,
) {
	return useQuery({
		queryKey: ["i18n", "table", lang, name, ids.length],
		enabled: enabled && ids.length > 0,
		staleTime: Number.POSITIVE_INFINITY,
		gcTime: 5 * 60_000,
		queryFn: async ({ signal }) => {
			const texts: Record<string, string> = {};
			for (let i = 0; i < ids.length; i += I18N_CHUNK) {
				const res = await api.i18nResolve(
					lang,
					ids.slice(i, i + I18N_CHUNK),
					signal,
				);
				Object.assign(texts, res.texts);
			}
			return texts;
		},
	});
}

function TablePage(): React.ReactElement {
	const { name } = Route.useParams();
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const query = useQuery(tableQuery(name));

	React.useEffect(() => {
		pushRecent("tables", `${name}.json`);
	}, [name]);

	const tabular = React.useMemo(
		() => (query.data === undefined ? null : toTabular(query.data)),
		[query.data],
	);
	const view = tabular ? (search.view ?? "grid") : "json";
	const i18nEnabled = search.i18n ?? true;

	const ids = React.useMemo(
		() => (tabular && i18nEnabled ? collectI18nIds(tabular.rows) : []),
		[tabular, i18nEnabled],
	);
	const i18n = useI18nMap(
		textLanguage,
		name,
		ids,
		i18nEnabled && view === "grid",
	);
	const resolve = React.useCallback(
		(id: string) => i18n.data?.[id],
		[i18n.data],
	);

	const sorting = React.useMemo(() => parseSort(search.sort), [search.sort]);
	const setSearch = (
		patch: Partial<ReturnType<typeof parseSearch>>,
		replace = true,
	) => void navigate({ search: (prev) => ({ ...prev, ...patch }), replace });

	const openRow =
		search.row !== undefined ? tabular?.rows[search.row] : undefined;

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b px-4 py-3">
				<div className="min-w-0">
					<h1 className="truncate font-semibold text-base tracking-tight">
						{name}
					</h1>
					<p className="text-muted-foreground text-xs">
						{query.data === undefined
							? "Loading table…"
							: tabular
								? `${formatNumber(tabular.rows.length)} rows · ${formatNumber(tabular.columns.length)} columns`
								: "Not tabular: shown as a JSON tree"}
					</p>
				</div>
				<div className="ms-auto flex items-center gap-2">
					{tabular && (
						<ToggleGroup
							aria-label="View"
							onValueChange={(value) => {
								const next = value[0];
								if (next === "grid" || next === "json")
									setSearch({ view: next === "grid" ? undefined : next });
							}}
							size="sm"
							value={[view]}
							variant="outline"
						>
							<ToggleGroupItem aria-label="Grid view" value="grid">
								<Table2Icon />
								Grid
							</ToggleGroupItem>
							<ToggleGroupItem aria-label="JSON view" value="json">
								<BracesIcon />
								JSON
							</ToggleGroupItem>
						</ToggleGroup>
					)}
					<Button
						aria-label="Download table JSON"
						render={
							<a
								download={`${name}.json`}
								href={`${API_URL}/api/tables/${encodeURIComponent(name)}`}
							/>
						}
						size="icon-sm"
						title="Download JSON"
						variant="ghost"
					>
						<DownloadIcon />
					</Button>
				</div>
			</div>

			{query.isError ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : query.data === undefined ? (
				<TableSkeleton />
			) : view === "grid" && tabular ? (
				<DataGrid
					data={tabular}
					filter={search.q ?? ""}
					i18nEnabled={i18nEnabled && Boolean(i18n.data)}
					onRowOpen={(_, index) => setSearch({ row: index }, false)}
					onSortingChange={(next) => setSearch({ sort: formatSort(next) })}
					resolveI18n={resolve}
					sorting={sorting}
					toolbarEnd={
						ids.length > 0 || !i18nEnabled ? (
							<Toggle
								aria-label="Resolve game text"
								onPressedChange={(pressed) =>
									setSearch({ i18n: pressed ? undefined : false })
								}
								pressed={i18nEnabled}
								size="sm"
								title={`Resolve text ids (${textLanguage})`}
								variant="outline"
							>
								{i18n.isFetching ? (
									<Spinner className="size-3.5" />
								) : (
									<LanguagesIcon />
								)}
								<span className="text-xs">{textLanguage}</span>
							</Toggle>
						) : null
					}
					toolbarStart={
						<SearchInput
							className="w-72 max-w-full"
							label="Filter rows"
							onCommit={(value) => setSearch({ q: value || undefined })}
							placeholder="Filter rows…"
							value={search.q ?? ""}
						/>
					}
				/>
			) : (
				<React.Suspense fallback={<ViewerFallback />}>
					<JsonViewer value={query.data} />
				</React.Suspense>
			)}

			<Sheet
				onOpenChange={(open) => {
					if (!open) setSearch({ row: undefined }, false);
				}}
				open={openRow !== undefined}
			>
				<SheetPopup className="w-full max-w-xl" side="right">
					<SheetHeader>
						<SheetTitle className="font-mono text-base">
							{openRow
								? String(
										openRow._key ??
											openRow.id ??
											`Row ${(search.row ?? 0) + 1}`,
									)
								: ""}
						</SheetTitle>
						<SheetDescription className="flex items-center gap-2">
							<span>
								{name} · row {(search.row ?? 0) + 1}
							</span>
							{openRow && (
								<Button
									className="ms-auto"
									onClick={() =>
										void copyText(JSON.stringify(openRow, null, 2), "Row JSON")
									}
									size="xs"
									variant="outline"
								>
									<CopyIcon />
									Copy JSON
								</Button>
							)}
						</SheetDescription>
					</SheetHeader>
					<SheetPanel className="flex min-h-0 flex-1 flex-col p-0">
						{openRow && (
							<React.Suspense fallback={<ViewerFallback />}>
								<JsonViewer value={openRow as Rec} />
							</React.Suspense>
						)}
					</SheetPanel>
				</SheetPopup>
			</Sheet>
		</div>
	);
}

function TableSkeleton(): React.ReactElement {
	return (
		<div aria-busy="true" className="flex flex-col gap-2 p-4">
			<Skeleton className="h-8 w-72" />
			{Array.from({ length: 14 }, (_, i) => (
				// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
				<Skeleton className="h-6" key={i} />
			))}
		</div>
	);
}
