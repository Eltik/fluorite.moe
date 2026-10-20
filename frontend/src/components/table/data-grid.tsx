/**
 * Virtualized data grid for decoded game tables (TanStack Table v9 + Virtual).
 *
 * Global filtering is done before rows reach the table (a cached lowercase
 * haystack per row), sorting and column visibility are Table features, and
 * only the visible rows are rendered.
 */
import {
	type ColumnDef,
	type ColumnVisibilityState,
	columnSizingFeature,
	columnVisibilityFeature,
	createSortedRowModel,
	rowSortingFeature,
	type SortingState,
	tableFeatures,
	useTable,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
	ArrowDownIcon,
	ArrowUpIcon,
	BracesIcon,
	BracketsIcon,
	Columns3Icon,
	CopyIcon,
	LanguagesIcon,
} from "lucide-react";
import * as React from "react";
import { Button } from "@/components/ui/button";
import {
	Menu,
	MenuCheckboxItem,
	MenuGroup,
	MenuGroupLabel,
	MenuItem,
	MenuPopup,
	MenuSeparator,
	MenuTrigger,
} from "@/components/ui/menu";
import {
	Popover,
	PopoverPopup,
	PopoverTitle,
	PopoverTrigger,
} from "@/components/ui/popover";
import { Spinner } from "@/components/ui/spinner";
import { copyText } from "@/lib/clipboard";
import { formatNumber } from "@/lib/format";
import { useRowHeight } from "@/lib/settings";
import { asI18n, cellText, type Rec, type Tabular } from "@/lib/table-shape";
import { cn } from "@/lib/utils";

const JsonViewer = React.lazy(() => import("@/components/viewers/json-viewer"));

/** Width of the row-number column, also the frozen key column's offset. */
const INDEX_WIDTH = 56;

const features = tableFeatures({
	rowSortingFeature,
	columnVisibilityFeature,
	columnSizingFeature,
	sortedRowModel: createSortedRowModel(),
});

type Features = typeof features;

export interface DataGridProps {
	data: Tabular;
	filter: string;
	sorting: SortingState;
	onSortingChange: (sorting: SortingState) => void;
	resolveI18n?: (id: string) => string | undefined;
	i18nEnabled: boolean;
	onRowOpen?: (row: Rec, index: number) => void;
	toolbarStart?: React.ReactNode;
	toolbarEnd?: React.ReactNode;
}

function measureColumn(key: string, rows: Rec[]): number {
	let longest = key.length + 2;
	for (const row of rows.slice(0, 200)) {
		const text = cellText(row[key]);
		if (text.length > longest) longest = text.length;
		if (longest > 48) break;
	}
	return Math.max(72, Math.min(360, Math.round(longest * 7.4 + 28)));
}

function sortValue(
	value: unknown,
	resolve?: (id: string) => string | undefined,
) {
	if (typeof value === "number" || typeof value === "boolean")
		return Number(value);
	if (value === null || value === undefined) return undefined;
	const ref = asI18n(value);
	if (ref)
		return (ref.text || resolve?.(ref.id) || "").toLowerCase() || undefined;
	if (typeof value === "string") return value.toLowerCase();
	if (Array.isArray(value)) return value.length;
	return undefined;
}

export function DataGrid({
	data,
	filter,
	sorting,
	onSortingChange,
	resolveI18n,
	i18nEnabled,
	onRowOpen,
	toolbarStart,
	toolbarEnd,
}: DataGridProps): React.ReactElement {
	const rowHeight = useRowHeight();
	const resolve = i18nEnabled ? resolveI18n : undefined;

	// ---- filtering (outside the table so the haystack can be cached)
	const haystacks = React.useRef(new WeakMap<Rec, string>());
	// biome-ignore lint/correctness/useExhaustiveDependencies: new resolver invalidates cache
	React.useEffect(() => {
		haystacks.current = new WeakMap();
	}, [resolve]);
	const needle = filter.trim().toLowerCase();
	const filtered = React.useMemo(() => {
		if (!needle) return data.rows;
		const tokens = needle.split(/\s+/);
		return data.rows.filter((row) => {
			let hay = haystacks.current.get(row);
			if (hay === undefined) {
				hay = data.columns
					.map((c) => cellText(row[c], resolve))
					.join("\u0000")
					.toLowerCase();
				haystacks.current.set(row, hay);
			}
			return tokens.every((t) => (hay as string).includes(t));
		});
	}, [data, needle, resolve]);

	// ---- columns
	const columns = React.useMemo<ColumnDef<Features, Rec>[]>(
		() =>
			data.columns.map((key) => ({
				id: key,
				accessorFn: (row: Rec) => sortValue(row[key], resolve),
				header: key,
				size: measureColumn(key, data.rows),
				sortUndefined: "last",
				cell: ({ row }) => <Cell resolve={resolve} value={row.original[key]} />,
			})),
		[data, resolve],
	);

	const [columnVisibility, setColumnVisibility] =
		React.useState<ColumnVisibilityState>({});
	// biome-ignore lint/correctness/useExhaustiveDependencies: reset when the table changes
	React.useEffect(() => setColumnVisibility({}), [data]);

	const table = useTable({
		features,
		columns,
		data: filtered,
		state: { sorting, columnVisibility },
		onSortingChange: (updater) =>
			onSortingChange(
				typeof updater === "function" ? updater(sorting) : updater,
			),
		onColumnVisibilityChange: setColumnVisibility,
		enableSortingRemoval: true,
	});

	const rows = table.getRowModel().rows;
	const visibleColumns = table.getVisibleLeafColumns();
	const frozenId = visibleColumns[0]?.id;
	const frozenWidth = visibleColumns[0]?.getSize() ?? 0;
	const template = `${INDEX_WIDTH}px ${visibleColumns.map((c) => `${c.getSize()}px`).join(" ")}`;
	const totalWidth =
		INDEX_WIDTH + visibleColumns.reduce((n, c) => n + c.getSize(), 0);

	// ---- virtualization
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const virtualizer = useVirtualizer({
		count: rows.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => rowHeight,
		overscan: 12,
		getItemKey: (index) => rows[index]?.id ?? index,
	});
	// biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when density changes
	React.useEffect(() => {
		virtualizer.measure();
	}, [rowHeight, virtualizer]);

	const hiddenCount = data.columns.length - visibleColumns.length;

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<div className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2">
				{toolbarStart}
				<div className="ms-auto flex items-center gap-1.5">
					{toolbarEnd}
					<Menu>
						<MenuTrigger render={<Button size="sm" variant="outline" />}>
							<Columns3Icon />
							Columns
							{hiddenCount > 0 && (
								<span className="text-muted-foreground text-xs tabular-nums">
									{visibleColumns.length}/{data.columns.length}
								</span>
							)}
						</MenuTrigger>
						<MenuPopup align="end" className="max-h-96 min-w-56">
							<MenuItem onClick={() => table.toggleAllColumnsVisible(true)}>
								Show all columns
							</MenuItem>
							<MenuSeparator />
							<MenuGroup>
								<MenuGroupLabel>Visible columns</MenuGroupLabel>
								{table.getAllLeafColumns().map((column) => (
									<MenuCheckboxItem
										checked={column.getIsVisible()}
										closeOnClick={false}
										key={column.id}
										onCheckedChange={(checked) =>
											column.toggleVisibility(checked)
										}
									>
										<span className="truncate font-mono text-xs">
											{column.id}
										</span>
									</MenuCheckboxItem>
								))}
							</MenuGroup>
						</MenuPopup>
					</Menu>
				</div>
			</div>

			<div
				aria-colcount={visibleColumns.length + 1}
				aria-label="Table rows"
				aria-rowcount={rows.length + 1}
				className="min-h-0 flex-1 overflow-auto text-sm"
				ref={scrollRef}
				role="table"
			>
				<div style={{ width: totalWidth, minWidth: "100%" }}>
					{/* Header */}
					<div
						className="sticky top-0 z-10 grid border-b bg-background/95 backdrop-blur-sm"
						role="row"
						style={{ gridTemplateColumns: template, height: rowHeight + 4 }}
					>
						<div
							className="sticky left-0 z-[1] flex items-center bg-background px-2 text-muted-foreground text-xs"
							role="columnheader"
						>
							#
						</div>
						{table.getHeaderGroups()[0]?.headers.map((header) => {
							const sorted = header.column.getIsSorted();
							return (
								<div
									aria-sort={
										sorted === "asc"
											? "ascending"
											: sorted === "desc"
												? "descending"
												: "none"
									}
									className={cn(
										"flex min-w-0 items-center border-s",
										header.column.id === frozenId &&
											"sticky z-[1] border-e bg-background",
									)}
									key={header.id}
									role="columnheader"
									style={
										header.column.id === frozenId
											? { left: INDEX_WIDTH }
											: undefined
									}
								>
									<button
										className="flex h-full w-full min-w-0 cursor-pointer items-center gap-1 px-2 text-start font-medium font-mono text-xs outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
										onClick={header.column.getToggleSortingHandler()}
										title={`Sort by ${header.column.id}`}
										type="button"
									>
										<span className="truncate">{header.column.id}</span>
										{sorted === "asc" && (
											<ArrowUpIcon className="size-3 shrink-0" />
										)}
										{sorted === "desc" && (
											<ArrowDownIcon className="size-3 shrink-0" />
										)}
									</button>
								</div>
							);
						})}
					</div>

					{/* Body */}
					<div
						className="relative"
						style={{ height: virtualizer.getTotalSize() }}
					>
						{virtualizer.getVirtualItems().map((item) => {
							const row = rows[item.index];
							if (!row) return null;
							return (
								<div
									aria-rowindex={item.index + 2}
									className={cn(
										"group/row absolute top-0 left-0 grid w-full border-b border-border/60 hover:bg-accent/60",
										onRowOpen && "cursor-pointer",
									)}
									key={item.key}
									onClick={() => onRowOpen?.(row.original, row.index)}
									onKeyDown={(event) => {
										if (event.key === "Enter")
											onRowOpen?.(row.original, row.index);
									}}
									role="row"
									style={{
										gridTemplateColumns: template,
										height: rowHeight,
										transform: `translateY(${item.start}px)`,
									}}
									tabIndex={onRowOpen ? 0 : undefined}
								>
									<div
										className="sticky left-0 z-[1] flex items-center bg-background px-2 text-muted-foreground text-xs tabular-nums group-hover/row:bg-accent"
										role="cell"
									>
										<span className="group-focus-within/row:hidden group-hover/row:hidden">
											{row.index + 1}
										</span>
										<button
											aria-label={`Copy row ${row.index + 1} as JSON`}
											className="hidden size-6 cursor-pointer items-center justify-center rounded-sm outline-none hover:bg-background hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring group-focus-within/row:flex group-hover/row:flex"
											onClick={(event) => {
												event.stopPropagation();
												void copyText(
													JSON.stringify(row.original, null, 2),
													"Row JSON",
												);
											}}
											title="Copy row as JSON"
											type="button"
										>
											<CopyIcon className="size-3.5" />
										</button>
									</div>
									{row.getVisibleCells().map((cell) => (
										<div
											className={cn(
												"flex min-w-0 items-center border-s border-border/60 px-2",
												cell.column.id === frozenId &&
													"sticky z-[1] border-e bg-background group-hover/row:bg-accent",
											)}
											key={cell.id}
											role="cell"
											style={
												cell.column.id === frozenId
													? { left: INDEX_WIDTH, width: frozenWidth }
													: undefined
											}
										>
											<table.FlexRender cell={cell} />
										</div>
									))}
								</div>
							);
						})}
					</div>
				</div>
			</div>

			<div className="flex h-8 shrink-0 items-center gap-3 border-t px-3 text-muted-foreground text-xs tabular-nums">
				<span>
					{needle
						? `${formatNumber(rows.length)} of ${formatNumber(data.rows.length)} rows`
						: `${formatNumber(data.rows.length)} rows`}
				</span>
				<span>
					{formatNumber(data.columns.length)} columns
					{hiddenCount > 0 && ` (${hiddenCount} hidden)`}
				</span>
				{i18nEnabled && (
					<span className="ms-auto inline-flex items-center gap-1">
						<LanguagesIcon className="size-3" /> game text resolved
					</span>
				)}
			</div>
		</div>
	);
}

function Cell({
	value,
	resolve,
}: {
	value: unknown;
	resolve?: (id: string) => string | undefined;
}): React.ReactElement {
	if (value === null || value === undefined) {
		return <span className="text-muted-foreground/50">—</span>;
	}
	if (typeof value === "number") {
		return (
			<span className="ms-auto font-mono text-xs tabular-nums">{value}</span>
		);
	}
	if (typeof value === "boolean") {
		return (
			<span
				className={cn(
					"font-mono text-xs",
					value ? "text-success-foreground" : "text-muted-foreground",
				)}
			>
				{String(value)}
			</span>
		);
	}
	if (typeof value === "string") {
		return (
			<span className="truncate" title={value.length > 40 ? value : undefined}>
				{value || <span className="text-muted-foreground">""</span>}
			</span>
		);
	}
	const ref = asI18n(value);
	if (ref) {
		const text = ref.text || resolve?.(ref.id);
		return text ? (
			<span className="truncate" title={`${text}\n(id ${ref.id})`}>
				{text}
			</span>
		) : (
			<span
				className="truncate font-mono text-muted-foreground text-xs"
				title={`i18n id ${ref.id}`}
			>
				{ref.id === "0" ? "—" : `#${ref.id}`}
			</span>
		);
	}
	if (typeof value === "object") return <NestedCell value={value} />;
	const text = cellText(value, resolve);
	return (
		<span
			className="truncate font-mono text-muted-foreground text-xs"
			title={text}
		>
			{text}
		</span>
	);
}

/** `{…} 3 keys` / `[…] 5` chip that opens the value as a JSON tree. */
function NestedCell({ value }: { value: object }): React.ReactElement {
	const isArray = Array.isArray(value);
	const size = isArray ? value.length : Object.keys(value).length;
	if (size === 0) {
		return (
			<span className="font-mono text-muted-foreground text-xs">
				{isArray ? "[]" : "{}"}
			</span>
		);
	}
	const Icon = isArray ? BracketsIcon : BracesIcon;
	const label = isArray
		? `${formatNumber(size)} item${size === 1 ? "" : "s"}`
		: `${formatNumber(size)} key${size === 1 ? "" : "s"}`;
	return (
		<Popover>
			<PopoverTrigger
				aria-label={`Show ${label}`}
				className="inline-flex h-5 max-w-full cursor-pointer items-center gap-1 rounded-sm border bg-muted/60 px-1.5 font-mono text-muted-foreground text-xs outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
				onClick={(event) => event.stopPropagation()}
				onKeyDown={(event) => event.stopPropagation()}
			>
				<Icon className="size-3 shrink-0" />
				<span className="truncate">{label}</span>
			</PopoverTrigger>
			<PopoverPopup
				className="w-[min(32rem,90vw)] [&_[data-slot=popover-viewport]]:p-0"
				onClick={(event) => event.stopPropagation()}
			>
				<PopoverTitle className="sr-only">Nested value</PopoverTitle>
				<div className="flex h-80 flex-col">
					<React.Suspense
						fallback={
							<div className="flex flex-1 items-center justify-center">
								<Spinner />
							</div>
						}
					>
						<JsonViewer value={value} />
					</React.Suspense>
				</div>
			</PopoverPopup>
		</Popover>
	);
}
