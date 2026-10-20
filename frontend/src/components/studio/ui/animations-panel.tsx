/**
 * The animations browser: Featured (curated groups) and All animations
 * (every clip, grouped by source), with search, hover prefetch, a loading
 * spinner on the row being fetched and a progress underline on the playing
 * row. Both tabs render through one virtualized list, so 700 clips cost the
 * same as 20. A card on the left on desktop; the body alone inside the
 * bottom sheet on phones.
 */
import { useVirtualizer } from "@tanstack/react-virtual";
import {
	FootprintsIcon,
	HandIcon,
	type LucideIcon,
	MessageSquareIcon,
	RepeatIcon,
	SearchIcon,
	SparklesIcon,
	SwordsIcon,
	TriangleAlertIcon,
	UserRoundIcon,
	XIcon,
} from "lucide-react";
import * as React from "react";
import type { AnimationController } from "@/components/studio/animation";
import { type ClipTab, setPrefs, usePrefs } from "@/components/studio/prefs";
import { GlassCard, IconButton } from "@/components/studio/ui/chrome";
import {
	useAnimationState,
	useClipTime,
} from "@/components/studio/ui/transport";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import {
	type ClipEntry,
	type ClipSource,
	clipLabel,
	clipStem,
	type FeaturedGroupId,
	featuredGroups,
	filterClips,
	groupBySource,
	matchesQuery,
} from "@/lib/clips";
import { cn } from "@/lib/utils";

const SOURCE_ICON: Record<ClipSource, LucideIcon> = {
	ui: UserRoundIcon,
	battle: SwordsIcon,
	"3c": FootprintsIcon,
	interact: HandIcon,
	dialog: MessageSquareIcon,
	other: SparklesIcon,
};

const FEATURED_ICON: Record<FeaturedGroupId, LucideIcon> = {
	menu: UserRoundIcon,
	battle: SwordsIcon,
	movement: FootprintsIcon,
	dialog: MessageSquareIcon,
};

type Row =
	| {
			kind: "header";
			id: string;
			label: string;
			count: number;
			icon: LucideIcon;
	  }
	| {
			kind: "clip";
			id: string;
			entry: ClipEntry;
			label: string;
			icon: LucideIcon;
	  };

const HEADER_HEIGHT = 32;
const ROW_HEIGHT = 46;

function buildRows(
	tab: ClipTab,
	entries: readonly ClipEntry[],
	query: string,
): Row[] {
	const rows: Row[] = [];
	if (tab === "featured") {
		for (const group of featuredGroups(entries)) {
			const items = query
				? group.items.filter((i) =>
						matchesQuery(`${i.label} ${i.entry.name} ${i.entry.file}`, query),
					)
				: group.items;
			if (!items.length) continue;
			const icon = FEATURED_ICON[group.id];
			rows.push({
				kind: "header",
				id: `h:${group.id}`,
				label: group.label,
				count: items.length,
				icon,
			});
			for (const item of items) {
				rows.push({
					kind: "clip",
					id: clipStem(item.entry.file),
					entry: item.entry,
					label: item.label,
					icon,
				});
			}
		}
		return rows;
	}
	for (const group of groupBySource(filterClips(entries, query))) {
		const icon = SOURCE_ICON[group.id as ClipSource];
		rows.push({
			kind: "header",
			id: `h:${group.id}`,
			label: group.label,
			count: group.items.length,
			icon,
		});
		for (const entry of group.items) {
			rows.push({
				kind: "clip",
				id: clipStem(entry.file),
				entry,
				label: clipLabel(entry),
				icon,
			});
		}
	}
	return rows;
}

export interface AnimationsBrowserProps {
	controller: AnimationController;
	onSelect: (stem: string) => void;
	/** Escape with an empty search (desktop card only). */
	onClose?: () => void;
	searchRef?: React.RefObject<HTMLInputElement | null>;
	/** Phone layout: the list takes a fixed share of the sheet. */
	mobile?: boolean;
}

export function AnimationsBrowser({
	controller,
	onSelect,
	onClose,
	searchRef,
	mobile = false,
}: AnimationsBrowserProps): React.ReactElement {
	const state = useAnimationState(controller);
	const prefs = usePrefs();
	const tab = prefs.clipTab;
	const [search, setSearch] = React.useState("");
	const deferred = React.useDeferredValue(search);
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const listRef = React.useRef<HTMLUListElement>(null);

	const rows = React.useMemo(
		() => buildRows(tab, controller.entries, deferred.trim()),
		[tab, controller, deferred],
	);
	const featuredEmpty =
		tab === "featured" && !deferred.trim() && rows.length === 0;

	const virtualizer = useVirtualizer({
		count: rows.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: (i) =>
			rows[i].kind === "header" ? HEADER_HEIGHT : ROW_HEIGHT,
		getItemKey: (i) => rows[i].id,
		overscan: 10,
	});

	// Keep the playing clip in view when the list changes (tab, search, new
	// model), not on every tick.
	const activeId = state.clip;
	// biome-ignore lint/correctness/useExhaustiveDependencies: scroll on list change only
	React.useEffect(() => {
		const i = rows.findIndex((r) => r.kind === "clip" && r.id === activeId);
		if (i >= 0) virtualizer.scrollToIndex(i, { align: "auto" });
	}, [rows]);

	const focusRow = React.useCallback(
		(index: number) => {
			virtualizer.scrollToIndex(index, { align: "auto" });
			requestAnimationFrame(() => {
				listRef.current
					?.querySelector<HTMLElement>(`[data-row="${index}"]`)
					?.focus();
			});
		},
		[virtualizer],
	);

	/** Next clip row from `from` in direction `dir` (skipping headers). */
	const nextClipRow = (from: number, dir: 1 | -1): number => {
		for (let i = from + dir; i >= 0 && i < rows.length; i += dir) {
			if (rows[i].kind === "clip") return i;
		}
		return -1;
	};
	const firstClipRow = () => nextClipRow(-1, 1);
	const lastClipRow = () => nextClipRow(rows.length, -1);

	const onListKeyDown = (e: React.KeyboardEvent<HTMLUListElement>) => {
		const el = (e.target as HTMLElement).closest<HTMLElement>("[data-row]");
		const current = el ? Number(el.dataset.row) : -1;
		let next = -1;
		switch (e.key) {
			case "ArrowDown":
				next = nextClipRow(current, 1);
				break;
			case "ArrowUp":
				next = current < 0 ? -1 : nextClipRow(current, -1);
				if (next < 0 && current >= 0) {
					e.preventDefault();
					searchRef?.current?.focus();
					return;
				}
				break;
			case "Home":
				next = firstClipRow();
				break;
			case "End":
				next = lastClipRow();
				break;
			default:
				return;
		}
		if (next < 0) return;
		e.preventDefault();
		focusRow(next);
	};

	const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
		if (e.key === "ArrowDown") {
			const first = firstClipRow();
			if (first >= 0) {
				e.preventDefault();
				focusRow(first);
			}
		} else if (e.key === "Escape") {
			if (search) {
				e.preventDefault();
				e.stopPropagation();
				setSearch("");
			} else if (onClose) {
				e.preventDefault();
				e.stopPropagation();
				onClose();
			}
		}
	};

	return (
		<div className="flex min-h-0 flex-1 flex-col gap-2">
			<Tabs
				onValueChange={(v) => setPrefs({ clipTab: v as ClipTab })}
				value={tab}
			>
				<TabsList aria-label="Animation lists" className="w-full" size="sm">
					<TabsTab value="featured">Featured</TabsTab>
					<TabsTab value="all">All animations</TabsTab>
				</TabsList>
			</Tabs>
			<div className="relative">
				<SearchIcon
					aria-hidden="true"
					className="pointer-events-none absolute start-2.5 top-1/2 z-10 size-4 -translate-y-1/2 text-muted-foreground"
				/>
				<Input
					aria-label="Search animations"
					className="ps-8"
					onChange={(e) => setSearch(e.target.value)}
					onKeyDown={onSearchKeyDown}
					placeholder={
						tab === "all"
							? `Search ${controller.entries.length} animations…`
							: "Search animations…"
					}
					ref={searchRef}
					size="sm"
					type="search"
					value={search}
				/>
			</div>
			<div
				className={cn(
					"-mx-1 relative overflow-y-auto overscroll-contain px-1",
					mobile ? "h-[52dvh]" : "min-h-0 flex-1",
				)}
				ref={scrollRef}
			>
				{rows.length === 0 ? (
					<div className="flex flex-col items-center gap-2 py-8 text-center text-muted-foreground text-sm">
						{featuredEmpty ? (
							<>
								<span>Nothing to feature for this model yet.</span>
								<Button
									onClick={() => setPrefs({ clipTab: "all" })}
									size="xs"
									variant="outline"
								>
									Browse all animations
								</Button>
							</>
						) : (
							<span>No match</span>
						)}
					</div>
				) : (
					<ul
						aria-label={
							tab === "featured" ? "Featured animations" : "All animations"
						}
						className="relative w-full"
						onKeyDown={onListKeyDown}
						ref={listRef}
						style={{ height: virtualizer.getTotalSize() }}
					>
						{virtualizer.getVirtualItems().map((v) => {
							const row = rows[v.index];
							const style = {
								height: v.size,
								transform: `translateY(${v.start}px)`,
							};
							if (row.kind === "header") {
								const Icon = row.icon;
								return (
									<li
										className="absolute inset-x-0 flex items-end px-2 pb-1"
										key={v.key}
										style={style}
									>
										<span className="flex w-full items-center gap-1.5 font-medium text-[0.6875rem] text-muted-foreground uppercase tracking-wide">
											<Icon aria-hidden="true" className="size-3.5" />
											{row.label}
											<span className="ms-auto font-mono normal-case tabular-nums">
												{row.count}
											</span>
										</span>
									</li>
								);
							}
							return (
								<li
									className="absolute inset-x-0 py-0.5"
									key={v.key}
									style={style}
								>
									<ClipRow
										active={row.id === state.clip}
										controller={controller}
										failed={row.id === state.failed}
										index={v.index}
										loading={row.id === state.loading}
										onSelect={onSelect}
										row={row}
									/>
								</li>
							);
						})}
					</ul>
				)}
			</div>
			<div className="flex min-h-7 items-center justify-between gap-3 border-t pt-2 text-muted-foreground text-xs">
				<span className="tabular-nums">
					{controller.entries.length} clip
					{controller.entries.length === 1 ? "" : "s"}
				</span>
				{controller.hasRootMotion && (
					<label className="flex items-center gap-2">
						<span>Root motion</span>
						<Switch
							checked={prefs.rootMotion}
							className="[--thumb-size:--spacing(4)] sm:[--thumb-size:--spacing(3.5)]"
							onCheckedChange={(rootMotion) => setPrefs({ rootMotion })}
						/>
					</label>
				)}
			</div>
		</div>
	);
}

function ClipRow({
	row,
	index,
	active,
	loading,
	failed,
	controller,
	onSelect,
}: {
	row: Extract<Row, { kind: "clip" }>;
	index: number;
	active: boolean;
	loading: boolean;
	failed: boolean;
	controller: AnimationController;
	onSelect: (stem: string) => void;
}): React.ReactElement {
	const Icon = row.icon;
	const { entry } = row;
	const prefetch = () => controller.prefetch(row.id);
	return (
		<button
			aria-busy={loading || undefined}
			aria-current={active ? "true" : undefined}
			className={cn(
				"group relative flex h-full w-full items-center gap-2.5 overflow-hidden rounded-lg px-2 text-start outline-none transition-colors hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring",
				active && "bg-primary/10 hover:bg-primary/14 dark:bg-primary/16",
			)}
			data-row={index}
			onClick={() => onSelect(row.id)}
			onFocus={prefetch}
			onPointerEnter={prefetch}
			title={entry.file}
			type="button"
		>
			<span
				className={cn(
					"flex size-7 shrink-0 items-center justify-center rounded-md bg-muted/70 text-muted-foreground",
					active && "bg-primary/15 text-primary",
					failed && "bg-destructive/10 text-destructive",
				)}
			>
				{loading ? (
					<Spinner className="size-3.5" />
				) : failed ? (
					<TriangleAlertIcon aria-hidden="true" className="size-3.5" />
				) : (
					<Icon aria-hidden="true" className="size-3.5" />
				)}
			</span>
			<span className="flex min-w-0 flex-1 flex-col leading-tight">
				<span className={cn("truncate text-sm", active && "font-medium")}>
					{row.label}
				</span>
				<span className="font-mono text-[0.6875rem] text-muted-foreground tabular-nums">
					{failed
						? "Couldn't load"
						: loading
							? "Loading…"
							: `${entry.duration.toFixed(1)} s`}
				</span>
			</span>
			{entry.loop && (
				<Badge
					className="gap-0.5 border-border/70 bg-transparent text-muted-foreground"
					size="sm"
					variant="outline"
				>
					<RepeatIcon aria-hidden="true" />
					loop
				</Badge>
			)}
			{active && <RowProgress controller={controller} />}
		</button>
	);
}

/** Thin playback progress under the active row. */
function RowProgress({
	controller,
}: {
	controller: AnimationController;
}): React.ReactElement | null {
	const state = useAnimationState(controller);
	const time = useClipTime(controller, state);
	if (!state.duration || state.loading) return null;
	const fraction = Math.min(1, Math.max(0, time / state.duration));
	return (
		<span
			aria-hidden="true"
			className="pointer-events-none absolute inset-x-2 bottom-0 h-0.5 overflow-hidden rounded-full bg-primary/15"
		>
			<span
				className="block h-full origin-left bg-primary"
				style={{ transform: `scaleX(${fraction})` }}
			/>
		</span>
	);
}

/** Desktop: the browser inside a left-side card with its own title and close. */
export function AnimationsCard({
	className,
	onClose,
	...props
}: AnimationsBrowserProps & {
	className?: string;
	onClose: () => void;
}): React.ReactElement {
	return (
		<GlassCard
			aria-label="Animations"
			className={cn(
				"flex h-[min(34rem,100%)] w-80 flex-col gap-2 rounded-2xl bg-popover/88 p-3 pt-2 dark:bg-popover/82",
				className,
			)}
			onKeyDown={(e) => {
				if (e.key === "Escape" && !e.defaultPrevented) {
					e.preventDefault();
					e.stopPropagation();
					onClose();
				}
			}}
			role="dialog"
		>
			<div className="flex items-center justify-between">
				<h2 className="ps-1 font-semibold text-sm">Animations</h2>
				<IconButton label="Close" onClick={onClose} shortcut="Esc">
					<XIcon />
				</IconButton>
			</div>
			<AnimationsBrowser onClose={onClose} {...props} />
		</GlassCard>
	);
}
