import { Link } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { BoxIcon } from "lucide-react";
import * as React from "react";
import { EntityImage } from "@/components/catalog/entity-image";
import { RarityStars, rarityStyle } from "@/components/catalog/rarity";
import type { CatalogKind } from "@/lib/api";
import type { ListItem } from "@/lib/catalog";
import { useSettings } from "@/lib/settings";
import { cn } from "@/lib/utils";

const GAP = 12;
const INFO = 66;

/**
 * Responsive, row-virtualized card grid. Columns follow the container width;
 * arrow keys / Home / End move a roving focus, Enter opens the card. Cards are
 * router links, so hovering or focusing one preloads its detail.
 */
export function CatalogGrid({
	kind,
	items,
	scrollElement,
	label,
}: {
	kind: CatalogKind;
	items: ListItem[];
	scrollElement: () => HTMLElement | null;
	label: string;
}): React.ReactElement {
	const { density } = useSettings();
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

	const preferred = density === "compact" ? 128 : 156;
	const min = width > 0 ? Math.min(preferred, (width - GAP) / 2) : preferred;
	const columns = Math.max(1, Math.floor((width + GAP) / (min + GAP)));
	const tile = width > 0 ? (width - GAP * (columns - 1)) / columns : min;
	const rowHeight = tile + INFO + GAP;
	const rows = Math.ceil(items.length / columns);

	const virtualizer = useVirtualizer({
		count: rows,
		getScrollElement: scrollElement,
		estimateSize: () => rowHeight,
		overscan: 3,
		scrollMargin,
	});

	// biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when the layout changes
	React.useEffect(() => {
		virtualizer.measure();
	}, [rowHeight, columns, virtualizer]);

	const [focus, setFocus] = React.useState(0);
	React.useEffect(() => {
		if (focus >= items.length) setFocus(0);
	}, [items.length, focus]);

	const focusItem = (index: number) => {
		const clamped = Math.max(0, Math.min(items.length - 1, index));
		setFocus(clamped);
		virtualizer.scrollToIndex(Math.floor(clamped / columns), { align: "auto" });
		requestAnimationFrame(() => {
			containerRef.current
				?.querySelector<HTMLElement>(`[data-card="${clamped}"]`)
				?.focus({ preventScroll: true });
		});
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		if (event.metaKey || event.ctrlKey || event.altKey) return;
		const moves: Record<string, number> = {
			ArrowRight: 1,
			ArrowLeft: -1,
			ArrowDown: columns,
			ArrowUp: -columns,
			PageDown: columns * 4,
			PageUp: -columns * 4,
		};
		let target: number | undefined;
		if (event.key === "Home") target = 0;
		else if (event.key === "End") target = items.length - 1;
		else if (moves[event.key] !== undefined)
			target = focus + (moves[event.key] as number);
		if (target === undefined) return;
		event.preventDefault();
		focusItem(target);
	};

	return (
		<section
			aria-label={label}
			className="relative w-full"
			onKeyDown={onKeyDown}
			ref={containerRef}
			style={{ height: virtualizer.getTotalSize() }}
		>
			{width > 0 &&
				virtualizer.getVirtualItems().map((row) => (
					<div
						className="absolute top-0 left-0 flex w-full"
						key={row.key}
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
								<div key={item.id} style={{ width: tile }}>
									<CatalogCard
										index={index}
										item={item}
										kind={kind}
										onFocus={() => setFocus(index)}
										tabIndex={index === focus ? 0 : -1}
									/>
								</div>
							);
						})}
					</div>
				))}
		</section>
	);
}

const CatalogCard = React.memo(function CatalogCard({
	kind,
	item,
	index,
	tabIndex,
	onFocus,
}: {
	kind: CatalogKind;
	item: ListItem;
	index: number;
	tabIndex: number;
	onFocus: () => void;
}): React.ReactElement {
	const tags = item.tags.slice(0, item.rarity ? 1 : 2);
	return (
		<Link
			className="group/card relative flex h-full flex-col overflow-hidden rounded-xl border bg-card text-card-foreground shadow-xs/5 outline-none transition-[translate,box-shadow,border-color] duration-150 hover:-translate-y-0.5 hover:border-foreground/16 hover:shadow-md/5 focus-visible:ring-2 focus-visible:ring-ring active:translate-y-0 motion-reduce:hover:translate-y-0"
			data-card={index}
			onFocus={onFocus}
			params={{ kind, id: item.id }}
			style={rarityStyle(item.rarity)}
			tabIndex={tabIndex}
			to="/catalog/$kind/$id"
		>
			<div
				className="relative min-h-0 flex-1 overflow-hidden bg-muted/60"
				style={
					item.rarity
						? {
								backgroundImage:
									"radial-gradient(120% 85% at 50% 110%, color-mix(in oklab, var(--rarity) 26%, transparent), transparent 72%)",
							}
						: undefined
				}
			>
				<EntityImage
					className="size-full p-2 text-2xl transition-transform duration-200 group-hover/card:scale-[1.04] motion-reduce:group-hover/card:scale-100"
					name={item.name}
					path={item.icon}
					size={256}
				/>
				{item.rarity ? (
					<span
						aria-hidden
						className="absolute inset-x-0 bottom-0 h-0.75 bg-(--rarity)"
					/>
				) : null}
				{item.hasModel && (
					<span
						className="absolute top-1.5 right-1.5 inline-flex size-5 items-center justify-center rounded-md bg-background/72 text-muted-foreground backdrop-blur-sm"
						title="Has a 3D model"
					>
						<BoxIcon aria-label="Has a 3D model" className="size-3" />
					</span>
				)}
			</div>
			<div
				className="flex shrink-0 flex-col gap-1 px-2.5 pt-2 pb-2.5"
				style={{ height: INFO }}
			>
				<span
					className="truncate font-medium text-sm leading-5"
					title={item.name}
				>
					{item.name}
				</span>
				<div className="flex min-w-0 items-center gap-1.5 overflow-hidden">
					<RarityStars className="shrink-0" rarity={item.rarity} />
					{tags.map((tag) => (
						<span
							className={cn(
								"truncate rounded-sm bg-muted px-1 text-[0.6875rem] text-muted-foreground leading-4",
							)}
							key={`${tag.kind}:${tag.code}`}
							title={tag.name}
						>
							{tag.name}
						</span>
					))}
					{tags.length === 0 && !item.rarity && (
						<span className="truncate font-mono text-[0.6875rem] text-muted-foreground/80">
							{item.id}
						</span>
					)}
				</div>
			</div>
		</Link>
	);
});
