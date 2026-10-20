/**
 * Model picker: kind tabs, search, and a horizontally scrolling tile strip
 * (desktop) or a grid (phone sheet). Tiles are real links; hovering or
 * focusing one prefetches its detail.
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronLeftIcon, ChevronRightIcon, SearchIcon } from "lucide-react";
import * as React from "react";
import {
	entryMatches,
	type StudioEntry,
	studioCatalogQuery,
	studioDetailQuery,
} from "@/components/studio/catalog-source";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { thumbUrl } from "@/lib/api";
import {
	STUDIO_KIND_LABELS,
	STUDIO_KINDS,
	type StudioKind,
} from "@/lib/studio";
import { cn } from "@/lib/utils";

export interface ModelPickerProps {
	tab: StudioKind;
	onTab: (kind: StudioKind) => void;
	currentKind?: StudioKind;
	currentId?: string;
	lang: string;
	layout: "strip" | "grid";
	bg?: string;
	searchRef?: React.Ref<HTMLInputElement>;
	onPicked?: () => void;
}

function initials(name: string): string {
	const words = name.split(/[\s_-]+/).filter(Boolean);
	return (
		words.length > 1 ? words[0][0] + words[1][0] : name.slice(0, 2)
	).toUpperCase();
}

function Tile({
	entry,
	current,
	layout,
	lang,
	bg,
	onPicked,
}: {
	entry: StudioEntry;
	current: boolean;
	layout: "strip" | "grid";
	lang: string;
	bg?: string;
	onPicked?: () => void;
}) {
	const queryClient = useQueryClient();
	const [failed, setFailed] = React.useState(false);
	const prefetch = () => {
		void queryClient.prefetchQuery(
			studioDetailQuery(entry.kind, entry.id, lang, () =>
				queryClient.ensureQueryData(studioCatalogQuery(entry.kind, lang)),
			),
		);
	};
	return (
		<Link
			aria-current={current ? "true" : undefined}
			className={cn(
				"group/tile relative flex shrink-0 snap-start flex-col overflow-hidden rounded-xl border bg-background/60 outline-none transition-[border-color,box-shadow,transform] hover:border-primary/40 focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.98] dark:bg-background/40",
				layout === "strip" ? "w-24" : "w-full",
				current && "border-primary ring-2 ring-primary/30",
			)}
			data-current={current ? "" : undefined}
			data-tile=""
			onClick={onPicked}
			onFocus={prefetch}
			onMouseEnter={prefetch}
			params={{ kind: entry.kind, id: entry.id }}
			search={{ bg }}
			title={entry.name}
			to="/studio/$kind/$id"
		>
			<div className="relative flex aspect-square items-center justify-center bg-gradient-to-b from-muted/50 to-muted">
				{entry.icon && !failed ? (
					<img
						alt=""
						className="size-full object-cover transition-transform duration-300 group-hover/tile:scale-105"
						decoding="async"
						draggable={false}
						loading="lazy"
						onError={() => setFailed(true)}
						src={thumbUrl(entry.icon, 128)}
					/>
				) : (
					<span
						aria-hidden="true"
						className="font-semibold text-lg text-muted-foreground/80 tracking-tight"
					>
						{initials(entry.name)}
					</span>
				)}
				{entry.rarity !== undefined && entry.rarity > 0 && (
					<span className="absolute end-1 top-1 rounded-sm bg-black/55 px-1 font-medium text-[0.625rem] text-amber-300 leading-4">
						{entry.rarity}★
					</span>
				)}
			</div>
			<span className="truncate px-1.5 py-1 text-center text-xs">
				{entry.name}
			</span>
		</Link>
	);
}

function useStripScroll(
	ref: React.RefObject<HTMLDivElement | null>,
	version: string,
) {
	const [edges, setEdges] = React.useState({ start: false, end: false });
	// biome-ignore lint/correctness/useExhaustiveDependencies: `version` re-measures when content changes
	React.useEffect(() => {
		const el = ref.current;
		if (!el) return;
		const update = () => {
			setEdges({
				start: el.scrollLeft > 4,
				end: el.scrollLeft + el.clientWidth < el.scrollWidth - 4,
			});
		};
		update();
		const ro = new ResizeObserver(update);
		ro.observe(el);
		el.addEventListener("scroll", update, { passive: true });
		// Vertical wheel scrolls the strip horizontally.
		const onWheel = (e: WheelEvent) => {
			if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
			if (el.scrollWidth <= el.clientWidth) return;
			e.preventDefault();
			el.scrollLeft += e.deltaY;
		};
		el.addEventListener("wheel", onWheel, { passive: false });
		return () => {
			ro.disconnect();
			el.removeEventListener("scroll", update);
			el.removeEventListener("wheel", onWheel);
		};
	}, [ref, version]);
	return edges;
}

export function ModelPicker({
	tab,
	onTab,
	currentKind,
	currentId,
	lang,
	layout,
	bg,
	searchRef,
	onPicked,
}: ModelPickerProps): React.ReactElement {
	const query = useQuery(studioCatalogQuery(tab, lang));
	const [search, setSearch] = React.useState("");
	const deferred = React.useDeferredValue(search);
	const listRef = React.useRef<HTMLDivElement>(null);

	const items = React.useMemo(() => {
		const all = query.data?.items ?? [];
		const needle = deferred.trim().toLowerCase();
		if (!needle) return all;
		return all.filter(
			(e) =>
				e.name.toLowerCase().includes(needle) ||
				e.id.toLowerCase().includes(needle) ||
				e.tags.some((t) => t.toLowerCase().includes(needle)),
		);
	}, [query.data, deferred]);

	const edges = useStripScroll(listRef, `${items.length}:${layout}`);

	// Centre the current model in view (without scrolling the page). Re-run on
	// the next frame too: the popup may still be animating in when this mounts.
	// biome-ignore lint/correctness/useExhaustiveDependencies: re-centre when the list changes
	React.useLayoutEffect(() => {
		const centre = () => {
			const el = listRef.current;
			const node = el?.querySelector<HTMLElement>("[data-current]");
			if (!el) return;
			if (!node) {
				el.scrollLeft = 0;
				el.scrollTop = 0;
				return;
			}
			const box = el.getBoundingClientRect();
			const r = node.getBoundingClientRect();
			if (layout === "strip") {
				el.scrollLeft += r.left + r.width / 2 - (box.left + box.width / 2);
			} else {
				el.scrollTop += r.top + r.height / 2 - (box.top + box.height / 2);
			}
		};
		centre();
		const frame = requestAnimationFrame(centre);
		// After the open transition, when the strip has its final size.
		const timer = window.setTimeout(centre, 240);
		return () => {
			cancelAnimationFrame(frame);
			window.clearTimeout(timer);
		};
	}, [items, layout]);

	const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
		const tiles = [
			...(listRef.current?.querySelectorAll<HTMLElement>("[data-tile]") ?? []),
		];
		const index = tiles.indexOf(document.activeElement as HTMLElement);
		if (index < 0) return;
		let next = index;
		if (e.key === "ArrowRight") next = index + 1;
		else if (e.key === "ArrowLeft") next = index - 1;
		else if (e.key === "Home") next = 0;
		else if (e.key === "End") next = tiles.length - 1;
		else if (
			layout === "grid" &&
			(e.key === "ArrowDown" || e.key === "ArrowUp")
		) {
			const cols = Math.max(
				1,
				Math.round(
					(listRef.current?.clientWidth ?? 1) / (tiles[0]?.clientWidth || 1),
				),
			);
			next = index + (e.key === "ArrowDown" ? cols : -cols);
		} else return;
		e.preventDefault();
		tiles[Math.max(0, Math.min(tiles.length - 1, next))]?.focus();
	};

	const scrollBy = (dir: 1 | -1) => {
		const el = listRef.current;
		if (el)
			el.scrollBy({ left: dir * el.clientWidth * 0.8, behavior: "smooth" });
	};

	const label = STUDIO_KIND_LABELS[tab];

	return (
		<div
			className={cn(
				"flex min-w-0 flex-col gap-3",
				layout === "strip" && "w-[min(56rem,calc(100vw-3rem))]",
			)}
		>
			<div className="flex flex-wrap items-center gap-2">
				<Tabs onValueChange={(v) => onTab(v as StudioKind)} value={tab}>
					<TabsList aria-label="Model type">
						{STUDIO_KINDS.map((k) => (
							<TabsTab key={k} value={k}>
								{STUDIO_KIND_LABELS[k]}
							</TabsTab>
						))}
					</TabsList>
				</Tabs>
				<div className="relative ms-auto min-w-40 flex-1 sm:max-w-64">
					<SearchIcon
						aria-hidden="true"
						className="pointer-events-none absolute start-2.5 top-1/2 z-10 size-4 -translate-y-1/2 text-muted-foreground"
					/>
					<Input
						aria-label={`Search ${label.toLowerCase()}`}
						className="ps-8"
						onChange={(e) => setSearch(e.target.value)}
						onKeyDown={(e) => {
							if (
								e.key === "ArrowDown" ||
								(e.key === "Enter" && items.length)
							) {
								e.preventDefault();
								listRef.current
									?.querySelector<HTMLElement>("[data-tile]")
									?.focus();
							}
						}}
						placeholder={`Search ${label.toLowerCase()}…`}
						ref={searchRef}
						size="sm"
						type="search"
						value={search}
					/>
				</div>
			</div>

			{query.data?.source === "meshes" && (
				<p className="text-muted-foreground text-xs">
					The {label.toLowerCase()} catalog isn't mapped yet, so this lists the
					model folders found in the extracted meshes.
				</p>
			)}

			<div className="relative">
				{query.isPending ? (
					<div
						className={cn(
							layout === "strip"
								? "flex gap-2 overflow-hidden"
								: "grid grid-cols-[repeat(auto-fill,minmax(5.5rem,1fr))] gap-2",
						)}
					>
						{Array.from({ length: 10 }, (_, i) => (
							<Skeleton
								className={cn(
									"aspect-[4/5] rounded-xl",
									layout === "strip" && "w-24 shrink-0",
								)}
								// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
								key={i}
							/>
						))}
					</div>
				) : query.isError ? (
					<div className="flex flex-col items-center gap-2 py-6 text-center text-muted-foreground text-sm">
						<span>Couldn't load the {label.toLowerCase()} list.</span>
						<Button
							onClick={() => void query.refetch()}
							size="sm"
							variant="outline"
						>
							Try again
						</Button>
					</div>
				) : items.length === 0 ? (
					<p className="py-8 text-center text-muted-foreground text-sm">
						{search
							? `Nothing matches “${search}”.`
							: `No ${label.toLowerCase()} yet.`}
					</p>
				) : (
					<>
						<div
							aria-label={label}
							className={cn(
								layout === "strip"
									? "flex snap-x scroll-px-1 gap-2 overflow-x-auto overscroll-x-contain px-1 py-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
									: "grid max-h-[52dvh] grid-cols-[repeat(auto-fill,minmax(5.5rem,1fr))] gap-2 overflow-y-auto p-1",
								layout === "strip" &&
									edges.start &&
									edges.end &&
									"[mask-image:linear-gradient(to_right,transparent,black_2.5rem,black_calc(100%-2.5rem),transparent)]",
								layout === "strip" &&
									edges.start &&
									!edges.end &&
									"[mask-image:linear-gradient(to_right,transparent,black_2.5rem)]",
								layout === "strip" &&
									!edges.start &&
									edges.end &&
									"[mask-image:linear-gradient(to_left,transparent,black_2.5rem)]",
							)}
							onKeyDown={onKeyDown}
							ref={listRef}
							role="group"
						>
							{items.map((entry) => (
								<Tile
									bg={bg}
									current={entryMatches(entry, currentKind, currentId)}
									entry={entry}
									key={entry.id}
									lang={lang}
									layout={layout}
									onPicked={onPicked}
								/>
							))}
						</div>
						{layout === "strip" && edges.start && (
							<Button
								aria-label="Scroll left"
								className="absolute start-0 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full shadow-md"
								onClick={() => scrollBy(-1)}
								size="icon-sm"
								tabIndex={-1}
								variant="outline"
							>
								<ChevronLeftIcon />
							</Button>
						)}
						{layout === "strip" && edges.end && (
							<Button
								aria-label="Scroll right"
								className="absolute end-0 top-1/2 translate-x-1/2 -translate-y-1/2 rounded-full shadow-md"
								onClick={() => scrollBy(1)}
								size="icon-sm"
								tabIndex={-1}
								variant="outline"
							>
								<ChevronRightIcon />
							</Button>
						)}
					</>
				)}
			</div>
			<p className="text-muted-foreground text-xs tabular-nums">
				{query.data
					? `${items.length.toLocaleString()} of ${query.data.items.length.toLocaleString()} ${label.toLowerCase()}`
					: " "}
			</p>
		</div>
	);
}
