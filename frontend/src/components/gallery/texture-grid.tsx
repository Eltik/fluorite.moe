import { useVirtualizer } from "@tanstack/react-virtual";
import { ImageOffIcon } from "lucide-react";
import * as React from "react";
import { thumbUrl } from "@/lib/api";
import { useSettings } from "@/lib/settings";
import { cn } from "@/lib/utils";

export interface GridItem {
	path: string;
	name: string;
}

const GAP = 8;
const CAPTION = 28;

/**
 * Responsive, row-virtualized thumbnail grid. Column count follows the
 * container width; arrow keys move focus in two dimensions.
 */
export function TextureGrid({
	items,
	onOpen,
	scrollElement,
	onEndReached,
	activePath,
}: {
	items: GridItem[];
	onOpen: (index: number) => void;
	/** The scrolling ancestor (the app content area). */
	scrollElement: () => HTMLElement | null;
	onEndReached?: () => void;
	activePath?: string;
}): React.ReactElement {
	const { density } = useSettings();
	const min = density === "compact" ? 112 : 148;
	const containerRef = React.useRef<HTMLDivElement>(null);
	const [width, setWidth] = React.useState(0);
	const [scrollMargin, setScrollMargin] = React.useState(0);

	React.useLayoutEffect(() => {
		const el = containerRef.current;
		if (!el) return;
		const update = () => {
			setWidth(el.clientWidth);
			const scroller = scrollElement();
			if (scroller) {
				setScrollMargin(
					el.getBoundingClientRect().top -
						scroller.getBoundingClientRect().top +
						scroller.scrollTop,
				);
			}
		};
		update();
		const observer = new ResizeObserver(update);
		observer.observe(el);
		return () => observer.disconnect();
	}, [scrollElement]);

	const columns = Math.max(1, Math.floor((width + GAP) / (min + GAP)));
	const tile = width > 0 ? (width - GAP * (columns - 1)) / columns : min;
	const rowHeight = tile + CAPTION + GAP;
	const rows = Math.ceil(items.length / columns);

	const virtualizer = useVirtualizer({
		count: rows,
		getScrollElement: scrollElement,
		estimateSize: () => rowHeight,
		overscan: 4,
		scrollMargin,
	});

	// biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when density changes
	React.useEffect(() => {
		virtualizer.measure();
	}, [rowHeight, virtualizer]);

	const virtualRows = virtualizer.getVirtualItems();
	const lastRow = virtualRows[virtualRows.length - 1];
	React.useEffect(() => {
		if (lastRow && lastRow.index >= rows - 3) onEndReached?.();
	}, [lastRow, rows, onEndReached]);

	const [focus, setFocus] = React.useState(0);
	const focusItem = (index: number) => {
		const clamped = Math.max(0, Math.min(items.length - 1, index));
		setFocus(clamped);
		virtualizer.scrollToIndex(Math.floor(clamped / columns), { align: "auto" });
		requestAnimationFrame(() => {
			containerRef.current
				?.querySelector<HTMLElement>(`[data-tile="${clamped}"]`)
				?.focus();
		});
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		const moves: Record<string, number> = {
			ArrowRight: 1,
			ArrowLeft: -1,
			ArrowDown: columns,
			ArrowUp: -columns,
		};
		const delta = moves[event.key];
		if (delta === undefined) return;
		event.preventDefault();
		focusItem(focus + delta);
	};

	return (
		<div
			aria-label="Textures"
			className="relative w-full"
			onKeyDown={onKeyDown}
			ref={containerRef}
			role="grid"
			style={{ height: virtualizer.getTotalSize() }}
		>
			{virtualRows.map((row) => (
				<div
					className="absolute top-0 left-0 flex w-full"
					key={row.key}
					role="row"
					style={{
						gap: GAP,
						height: rowHeight - GAP,
						transform: `translateY(${row.start - virtualizer.options.scrollMargin}px)`,
					}}
				>
					{Array.from({ length: columns }, (_, col) => {
						const index = row.index * columns + col;
						const item = items[index];
						if (!item)
							// biome-ignore lint/suspicious/noArrayIndexKey: filler cells are positional
							return <div key={col} style={{ width: tile }} />;
						return (
							<div key={item.path} role="gridcell" style={{ width: tile }}>
								<Tile
									active={item.path === activePath}
									index={index}
									item={item}
									onFocus={() => setFocus(index)}
									onOpen={() => onOpen(index)}
									size={tile}
									tabIndex={index === focus ? 0 : -1}
								/>
							</div>
						);
					})}
				</div>
			))}
		</div>
	);
}

const Tile = React.memo(function Tile({
	item,
	index,
	size,
	active,
	tabIndex,
	onOpen,
	onFocus,
}: {
	item: GridItem;
	index: number;
	size: number;
	active: boolean;
	tabIndex: number;
	onOpen: () => void;
	onFocus: () => void;
}) {
	const [state, setState] = React.useState<"loading" | "ready" | "error">(
		"loading",
	);
	const thumbSize = size > 200 ? 512 : 256;
	return (
		<button
			aria-label={item.name}
			className={cn(
				"group flex w-full cursor-pointer flex-col gap-1 rounded-lg text-start outline-none",
			)}
			data-tile={index}
			onClick={onOpen}
			onFocus={onFocus}
			tabIndex={tabIndex}
			title={item.path}
			type="button"
		>
			<span
				className={cn(
					"relative flex w-full items-center justify-center overflow-hidden rounded-lg border bg-checker transition-shadow group-hover:ring-2 group-hover:ring-ring/40 group-focus-visible:ring-2 group-focus-visible:ring-ring",
					active && "ring-2 ring-ring",
				)}
				style={{ height: size }}
			>
				{state === "error" ? (
					<ImageOffIcon className="size-5 text-muted-foreground" />
				) : (
					<img
						alt={item.name}
						className={cn(
							"max-h-full max-w-full object-contain transition-opacity duration-300",
							state === "loading" ? "opacity-0" : "opacity-100",
						)}
						decoding="async"
						draggable={false}
						loading="lazy"
						onError={() => setState("error")}
						onLoad={() => setState("ready")}
						src={thumbUrl(item.path, thumbSize)}
					/>
				)}
				{state === "loading" && (
					<span className="absolute inset-0 animate-pulse bg-muted" />
				)}
			</span>
			<span className="truncate px-0.5 text-muted-foreground text-xs group-hover:text-foreground">
				{item.name}
			</span>
		</button>
	);
});
