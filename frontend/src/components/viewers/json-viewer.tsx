/**
 * Virtualized, lazily expanded JSON tree for multi-MB documents.
 *
 * Only expanded nodes are flattened into rows, and only visible rows are
 * rendered, so a 300k-key object costs one flatten pass on expand and a
 * screenful of DOM. Search walks the whole value once per query and reveals
 * matches by expanding their ancestors on demand.
 */
import { useVirtualizer } from "@tanstack/react-virtual";
import {
	ChevronDownIcon,
	ChevronRightIcon,
	ChevronsDownUpIcon,
	ChevronsUpDownIcon,
	ChevronUpIcon,
	CopyIcon,
	FileJsonIcon,
	ListTreeIcon,
} from "lucide-react";
import * as React from "react";
import { SearchInput } from "@/components/search-input";
import { Button } from "@/components/ui/button";
import { Toggle } from "@/components/ui/toggle";
import { copyText } from "@/lib/clipboard";
import { formatNumber } from "@/lib/format";
import { useRowHeight } from "@/lib/settings";
import { cn } from "@/lib/utils";

const CodeViewer = React.lazy(() => import("./code-viewer"));

type Segment = string | number;
type Container = Record<string, unknown> | unknown[];

interface Row {
	id: string;
	depth: number;
	key: Segment | null;
	value: unknown;
	path: Segment[];
	expandable: boolean;
	size: number;
}

const SEP = "\u001f";
const MAX_MATCHES = 10_000;
const AUTO_EXPAND_ALL_LIMIT = 2_000;

function idOf(path: Segment[]): string {
	return path.length === 0 ? "$" : `$${SEP}${path.join(SEP)}`;
}

function isContainer(value: unknown): value is Container {
	return typeof value === "object" && value !== null;
}

function sizeOf(value: Container): number {
	return Array.isArray(value) ? value.length : Object.keys(value).length;
}

function entriesOf(value: Container): [Segment, unknown][] {
	if (Array.isArray(value)) return value.map((v, i) => [i, v]);
	return Object.entries(value);
}

function flatten(root: unknown, expanded: ReadonlySet<string>): Row[] {
	const rows: Row[] = [];
	const visit = (key: Segment | null, value: unknown, path: Segment[]) => {
		const expandable = isContainer(value) && sizeOf(value) > 0;
		const id = idOf(path);
		rows.push({
			id,
			depth: path.length,
			key,
			value,
			path,
			expandable,
			size: isContainer(value) ? sizeOf(value) : 0,
		});
		if (expandable && expanded.has(id)) {
			for (const [k, v] of entriesOf(value as Container)) {
				visit(k, v, [...path, k]);
			}
		}
	};
	visit(null, root, []);
	return rows;
}

function countNodes(value: unknown, limit: number): number {
	let count = 0;
	const stack: unknown[] = [value];
	while (stack.length && count <= limit) {
		const v = stack.pop();
		count++;
		if (isContainer(v)) for (const [, child] of entriesOf(v)) stack.push(child);
	}
	return count;
}

function allContainerIds(value: unknown): Set<string> {
	const ids = new Set<string>();
	const visit = (v: unknown, path: Segment[]) => {
		if (!isContainer(v)) return;
		ids.add(idOf(path));
		for (const [k, child] of entriesOf(v)) visit(child, [...path, k]);
	};
	visit(value, []);
	return ids;
}

function findMatches(root: unknown, query: string): Segment[][] {
	const needle = query.toLowerCase();
	const out: Segment[][] = [];
	const stack: [unknown, Segment[]][] = [[root, []]];
	// Depth-first in document order: push children reversed.
	while (stack.length && out.length < MAX_MATCHES) {
		const [value, path] = stack.pop() as [unknown, Segment[]];
		const key = path[path.length - 1];
		const keyHit =
			typeof key === "string" && key.toLowerCase().includes(needle);
		const valueHit =
			!isContainer(value) && String(value).toLowerCase().includes(needle);
		if (path.length > 0 && (keyHit || valueHit)) out.push(path);
		if (isContainer(value)) {
			const entries = entriesOf(value);
			for (let i = entries.length - 1; i >= 0; i--) {
				const [k, v] = entries[i] as [Segment, unknown];
				stack.push([v, [...path, k]]);
			}
		}
	}
	return out;
}

const IDENT = /^[A-Za-z_$][\w$]*$/;

export function formatPath(path: Segment[]): string {
	let out = "$";
	for (const seg of path) {
		if (typeof seg === "number") out += `[${seg}]`;
		else if (IDENT.test(seg)) out += `.${seg}`;
		else out += `[${JSON.stringify(seg)}]`;
	}
	return out;
}

export interface JsonViewerProps {
	value: unknown;
	/** Original source text, used by the raw view. */
	text?: string;
	className?: string;
	/** Extra controls rendered at the end of the toolbar. */
	toolbarExtra?: React.ReactNode;
}

export default function JsonViewer({
	value,
	text,
	className,
	toolbarExtra,
}: JsonViewerProps): React.ReactElement {
	const rowHeight = useRowHeight();
	const [raw, setRaw] = React.useState(false);
	const [expanded, setExpanded] = React.useState<Set<string>>(() => {
		if (countNodes(value, AUTO_EXPAND_ALL_LIMIT) <= AUTO_EXPAND_ALL_LIMIT) {
			return allContainerIds(value);
		}
		const initial = new Set<string>(["$"]);
		// Expand a lone top-level child too (common `{ "array": [...] }` shape).
		if (isContainer(value) && sizeOf(value) === 1) {
			const [first] = entriesOf(value);
			if (first) initial.add(idOf([first[0]]));
		}
		return initial;
	});
	const rows = React.useMemo(() => flatten(value, expanded), [value, expanded]);
	const indexById = React.useMemo(() => {
		const map = new Map<string, number>();
		rows.forEach((row, i) => {
			map.set(row.id, i);
		});
		return map;
	}, [rows]);

	const [active, setActive] = React.useState(0);
	const activeRow = rows[Math.min(active, rows.length - 1)];

	// ---- search
	const [query, setQuery] = React.useState("");
	const matches = React.useMemo(
		() => (query.trim() ? findMatches(value, query.trim()) : []),
		[value, query],
	);
	const matchIds = React.useMemo(
		() => new Set(matches.map((p) => idOf(p))),
		[matches],
	);
	const [matchIndex, setMatchIndex] = React.useState(0);
	const pendingScroll = React.useRef<string | null>(null);

	const reveal = React.useCallback((path: Segment[]) => {
		setExpanded((prev) => {
			const next = new Set(prev);
			for (let i = 0; i < path.length; i++) next.add(idOf(path.slice(0, i)));
			return next;
		});
		pendingScroll.current = idOf(path);
	}, []);

	const goToMatch = React.useCallback(
		(index: number) => {
			if (matches.length === 0) return;
			const wrapped = (index + matches.length) % matches.length;
			setMatchIndex(wrapped);
			const path = matches[wrapped];
			if (path) reveal(path);
		},
		[matches, reveal],
	);

	React.useEffect(() => {
		setMatchIndex(0);
		const first = matches[0];
		if (first) reveal(first);
	}, [matches, reveal]);

	// ---- virtualizer
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const virtualizer = useVirtualizer({
		count: rows.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => rowHeight,
		overscan: 16,
	});

	// biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when density changes
	React.useEffect(() => {
		virtualizer.measure();
	}, [rowHeight, virtualizer]);

	React.useEffect(() => {
		const target = pendingScroll.current;
		if (!target) return;
		const index = indexById.get(target);
		if (index === undefined) return;
		pendingScroll.current = null;
		setActive(index);
		virtualizer.scrollToIndex(index, { align: "center" });
	}, [indexById, virtualizer]);

	const toggle = (row: Row) => {
		if (!row.expandable) return;
		setExpanded((prev) => {
			const next = new Set(prev);
			if (next.has(row.id)) next.delete(row.id);
			else next.add(row.id);
			return next;
		});
	};

	const moveTo = (index: number) => {
		const clamped = Math.max(0, Math.min(rows.length - 1, index));
		setActive(clamped);
		virtualizer.scrollToIndex(clamped);
	};

	const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
		const row = rows[active];
		if (!row) return;
		const isOpen = expanded.has(row.id);
		switch (event.key) {
			case "ArrowDown":
				moveTo(active + 1);
				break;
			case "ArrowUp":
				moveTo(active - 1);
				break;
			case "PageDown":
				moveTo(active + 20);
				break;
			case "PageUp":
				moveTo(active - 20);
				break;
			case "Home":
				moveTo(0);
				break;
			case "End":
				moveTo(rows.length - 1);
				break;
			case "ArrowRight":
				if (row.expandable && !isOpen) toggle(row);
				else moveTo(active + 1);
				break;
			case "ArrowLeft":
				if (row.expandable && isOpen) toggle(row);
				else if (row.path.length > 0) {
					const parent = indexById.get(idOf(row.path.slice(0, -1)));
					if (parent !== undefined) moveTo(parent);
				}
				break;
			case "Enter":
			case " ":
				toggle(row);
				break;
			case "c":
				if (event.metaKey || event.ctrlKey) return;
				void copyText(formatPath(row.path), "Path");
				break;
			default:
				return;
		}
		event.preventDefault();
	};

	const treeId = React.useId();
	const big = React.useMemo(() => countNodes(value, 50_000) > 50_000, [value]);

	return (
		<div className={cn("flex min-h-0 flex-1 flex-col", className)}>
			<div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b px-2 py-1.5">
				{!raw && (
					<>
						<SearchInput
							className="w-56 max-w-full"
							delay={250}
							label="Search keys and values"
							onCommit={setQuery}
							onKeyDown={(event) => {
								if (event.key === "Enter") {
									event.preventDefault();
									goToMatch(matchIndex + (event.shiftKey ? -1 : 1));
								}
							}}
							placeholder="Search keys and values…"
							value={query}
						/>
						{query.trim() && (
							<span className="flex items-center gap-0.5 text-muted-foreground text-xs tabular-nums">
								<span aria-live="polite" className="px-1">
									{matches.length === 0
										? "No matches"
										: `${matchIndex + 1} / ${formatNumber(matches.length)}${
												matches.length >= MAX_MATCHES ? "+" : ""
											}`}
								</span>
								<Button
									aria-label="Previous match"
									disabled={matches.length === 0}
									onClick={() => goToMatch(matchIndex - 1)}
									size="icon-xs"
									variant="ghost"
								>
									<ChevronUpIcon />
								</Button>
								<Button
									aria-label="Next match"
									disabled={matches.length === 0}
									onClick={() => goToMatch(matchIndex + 1)}
									size="icon-xs"
									variant="ghost"
								>
									<ChevronDownIcon />
								</Button>
							</span>
						)}
						<div className="ms-auto flex items-center gap-0.5">
							<Button
								aria-label="Expand all"
								disabled={big}
								onClick={() => setExpanded(allContainerIds(value))}
								size="icon-sm"
								title={big ? "Too large to expand everything" : "Expand all"}
								variant="ghost"
							>
								<ChevronsUpDownIcon />
							</Button>
							<Button
								aria-label="Collapse all"
								onClick={() => {
									setExpanded(new Set(["$"]));
									moveTo(0);
								}}
								size="icon-sm"
								title="Collapse all"
								variant="ghost"
							>
								<ChevronsDownUpIcon />
							</Button>
						</div>
					</>
				)}
				<div className={cn("flex items-center gap-1", raw && "ms-auto")}>
					{toolbarExtra}
					<Toggle
						aria-label={raw ? "Show tree" : "Show raw JSON"}
						onPressedChange={setRaw}
						pressed={raw}
						size="sm"
						title={raw ? "Show tree" : "Show raw JSON"}
					>
						{raw ? <ListTreeIcon /> : <FileJsonIcon />}
						<span className="text-xs">{raw ? "Tree" : "Raw"}</span>
					</Toggle>
				</div>
			</div>

			{raw ? (
				<React.Suspense fallback={<div className="flex-1 bg-code" />}>
					<CodeViewer
						code={text ?? JSON.stringify(value, null, 2)}
						lang="json"
					/>
				</React.Suspense>
			) : (
				<>
					<div
						aria-activedescendant={
							activeRow ? `${treeId}-${active}` : undefined
						}
						aria-label="JSON tree"
						className="min-h-0 flex-1 overflow-auto bg-code font-mono text-[0.8125rem] outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
						onKeyDown={onKeyDown}
						ref={scrollRef}
						role="tree"
						tabIndex={0}
					>
						<div
							className="relative w-max min-w-full"
							style={{ height: virtualizer.getTotalSize() }}
						>
							{virtualizer.getVirtualItems().map((item) => {
								const row = rows[item.index];
								if (!row) return null;
								return (
									<JsonRow
										height={rowHeight}
										id={`${treeId}-${item.index}`}
										isActive={item.index === active}
										isCurrentMatch={
											matches[matchIndex] !== undefined &&
											idOf(matches[matchIndex] as Segment[]) === row.id
										}
										isMatch={matchIds.has(row.id)}
										isOpen={expanded.has(row.id)}
										key={row.id}
										onSelect={() => setActive(item.index)}
										onToggle={() => toggle(row)}
										row={row}
										start={item.start}
									/>
								);
							})}
						</div>
					</div>
					<div className="flex h-8 shrink-0 items-center gap-2 border-t px-3 text-muted-foreground text-xs">
						{activeRow && (
							<>
								<code
									className="min-w-0 truncate"
									title={formatPath(activeRow.path)}
								>
									{formatPath(activeRow.path)}
								</code>
								<Button
									aria-label="Copy path"
									className="shrink-0"
									onClick={() =>
										void copyText(formatPath(activeRow.path), "Path")
									}
									size="icon-xs"
									title="Copy path (c)"
									variant="ghost"
								>
									<CopyIcon />
								</Button>
								{!isContainer(activeRow.value) && (
									<Button
										className="shrink-0"
										onClick={() =>
											void copyText(String(activeRow.value), "Value")
										}
										size="xs"
										variant="ghost"
									>
										Copy value
									</Button>
								)}
							</>
						)}
						<span className="ms-auto shrink-0 tabular-nums">
							{formatNumber(rows.length)} rows
						</span>
					</div>
				</>
			)}
		</div>
	);
}

const JsonRow = React.memo(function JsonRow({
	row,
	id,
	start,
	height,
	isOpen,
	isActive,
	isMatch,
	isCurrentMatch,
	onToggle,
	onSelect,
}: {
	row: Row;
	id: string;
	start: number;
	height: number;
	isOpen: boolean;
	isActive: boolean;
	isMatch: boolean;
	isCurrentMatch: boolean;
	onToggle: () => void;
	onSelect: () => void;
}): React.ReactElement {
	return (
		// biome-ignore lint/a11y/useFocusableInteractive lint/a11y/useKeyWithClickEvents: focus and keys are handled by the tree (aria-activedescendant)
		<div
			aria-expanded={row.expandable ? isOpen : undefined}
			aria-level={row.depth + 1}
			aria-selected={isActive}
			className={cn(
				"absolute top-0 left-0 flex w-full cursor-default items-center whitespace-nowrap pe-6 hover:bg-code-highlight",
				isActive && "bg-accent",
				isMatch && "bg-amber-500/10",
				isCurrentMatch && "bg-amber-500/25",
			)}
			id={id}
			onClick={() => {
				onSelect();
				if (row.expandable) onToggle();
			}}
			role="treeitem"
			style={{
				height,
				transform: `translateY(${start}px)`,
				paddingInlineStart: 8 + row.depth * 16,
			}}
		>
			<span className="inline-flex size-4 shrink-0 items-center justify-center text-muted-foreground">
				{row.expandable &&
					(isOpen ? (
						<ChevronDownIcon className="size-3.5" />
					) : (
						<ChevronRightIcon className="size-3.5" />
					))}
			</span>
			{row.key !== null && (
				<>
					<span
						className={cn(
							"ms-1",
							typeof row.key === "number"
								? "text-muted-foreground/70"
								: "text-foreground",
						)}
					>
						{typeof row.key === "number" ? row.key : row.key}
					</span>
					<span className="me-2 text-muted-foreground/60">:</span>
				</>
			)}
			<ValuePreview isOpen={isOpen} row={row} />
		</div>
	);
});

function ValuePreview({ row, isOpen }: { row: Row; isOpen: boolean }) {
	const { value } = row;
	if (Array.isArray(value)) {
		return (
			<span className="text-muted-foreground">
				{isOpen ? "[" : row.size === 0 ? "[]" : "[…]"}
				<span className="ms-2 text-muted-foreground/60 text-xs">
					{formatNumber(row.size)} {row.size === 1 ? "item" : "items"}
				</span>
			</span>
		);
	}
	if (isContainer(value)) {
		return (
			<span className="text-muted-foreground">
				{isOpen ? "{" : row.size === 0 ? "{}" : "{…}"}
				<span className="ms-2 text-muted-foreground/60 text-xs">
					{formatNumber(row.size)} {row.size === 1 ? "key" : "keys"}
				</span>
			</span>
		);
	}
	return <Primitive value={value} />;
}

export function Primitive({ value }: { value: unknown }): React.ReactElement {
	if (value === null)
		return <span className="text-muted-foreground italic">null</span>;
	if (typeof value === "string") {
		const shown = value.length > 400 ? `${value.slice(0, 400)}…` : value;
		return (
			<span
				className="text-emerald-700 dark:text-emerald-400"
				title={value.length > 60 ? value.slice(0, 2000) : undefined}
			>
				"{shown.replace(/\n/g, "↵")}"
			</span>
		);
	}
	if (typeof value === "number")
		return <span className="text-sky-700 dark:text-sky-400">{value}</span>;
	if (typeof value === "boolean")
		return (
			<span className="text-violet-700 dark:text-violet-400">
				{String(value)}
			</span>
		);
	return <span>{String(value)}</span>;
}
