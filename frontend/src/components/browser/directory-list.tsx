import { Link, type LinkProps } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
	BoxIcon,
	BracesIcon,
	CornerLeftUpIcon,
	FileCodeIcon,
	FileIcon,
	FileTextIcon,
	FolderIcon,
	ImageIcon,
	type LucideIcon,
} from "lucide-react";
import * as React from "react";
import type { TreeEntry } from "@/lib/api";
import { extname, formatBytes, formatNumber } from "@/lib/format";
import { useRowHeight } from "@/lib/settings";
import { cn } from "@/lib/utils";

const EXT_ICON: Record<string, LucideIcon> = {
	png: ImageIcon,
	jpg: ImageIcon,
	webp: ImageIcon,
	glb: BoxIcon,
	json: BracesIcon,
	lua: FileCodeIcon,
	txt: FileTextIcon,
	xml: FileCodeIcon,
};

export function entryIcon(entry: Pick<TreeEntry, "kind" | "name">): LucideIcon {
	if (entry.kind === "dir") return FolderIcon;
	return EXT_ICON[extname(entry.name)] ?? FileIcon;
}

export type EntryLink = Pick<LinkProps, "to" | "params" | "search" | "replace">;

export interface DirectoryListHandle {
	focusFirst: () => void;
}

/**
 * Virtualized listing with roving focus: arrow keys move between rows,
 * Enter follows the row link, Backspace / Left goes to the parent folder.
 */
export const DirectoryList = React.forwardRef<
	DirectoryListHandle,
	{
		entries: TreeEntry[];
		selectedPath?: string;
		linkFor: (entry: TreeEntry) => EntryLink;
		parentLink?: EntryLink;
		label: string;
		className?: string;
		emptyMessage?: React.ReactNode;
	}
>(function DirectoryList(
	{
		entries,
		selectedPath,
		linkFor,
		parentLink,
		label,
		className,
		emptyMessage,
	},
	ref,
) {
	const rowHeight = useRowHeight();
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const count = entries.length + (parentLink ? 1 : 0);
	const offset = parentLink ? 1 : 0;

	const selectedIndex = React.useMemo(() => {
		if (!selectedPath) return -1;
		const i = entries.findIndex((e) => e.path === selectedPath);
		return i === -1 ? -1 : i + offset;
	}, [entries, selectedPath, offset]);

	const [focusIndex, setFocusIndex] = React.useState(
		Math.max(0, selectedIndex),
	);

	const virtualizer = useVirtualizer({
		count,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => rowHeight,
		overscan: 12,
	});

	// biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when density changes
	React.useEffect(() => {
		virtualizer.measure();
	}, [rowHeight, virtualizer]);

	// Bring the selected entry into view when it changes (deep links, back/forward).
	React.useEffect(() => {
		if (selectedIndex >= 0) {
			setFocusIndex(selectedIndex);
			virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
		}
	}, [selectedIndex, virtualizer]);

	// Clamp focus when the listing shrinks (filtering).
	React.useEffect(() => {
		setFocusIndex((i) => Math.min(i, Math.max(0, count - 1)));
	}, [count]);

	const focusRow = React.useCallback(
		(index: number) => {
			const clamped = Math.max(0, Math.min(count - 1, index));
			setFocusIndex(clamped);
			virtualizer.scrollToIndex(clamped, { align: "auto" });
			// Wait a frame for the row to be rendered before focusing it.
			requestAnimationFrame(() => {
				scrollRef.current
					?.querySelector<HTMLElement>(`[data-row="${clamped}"]`)
					?.focus();
			});
		},
		[count, virtualizer],
	);

	React.useImperativeHandle(ref, () => ({ focusFirst: () => focusRow(0) }), [
		focusRow,
	]);

	const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
		const page = Math.max(
			1,
			Math.floor((scrollRef.current?.clientHeight ?? 300) / rowHeight) - 1,
		);
		switch (event.key) {
			case "ArrowDown":
				focusRow(focusIndex + 1);
				break;
			case "ArrowUp":
				focusRow(focusIndex - 1);
				break;
			case "PageDown":
				focusRow(focusIndex + page);
				break;
			case "PageUp":
				focusRow(focusIndex - page);
				break;
			case "Home":
				focusRow(0);
				break;
			case "End":
				focusRow(count - 1);
				break;
			case "ArrowRight": {
				const entry = entries[focusIndex - offset];
				if (entry?.kind !== "dir") return;
				scrollRef.current
					?.querySelector<HTMLElement>(`[data-row="${focusIndex}"]`)
					?.click();
				break;
			}
			case "ArrowLeft":
			case "Backspace":
				if (!parentLink) return;
				scrollRef.current
					?.querySelector<HTMLElement>('[data-row="0"]')
					?.click();
				break;
			default:
				return;
		}
		event.preventDefault();
	};

	if (count === 0) {
		return (
			<div className="flex flex-1 items-center justify-center p-6 text-center text-muted-foreground text-sm">
				{emptyMessage ?? "This folder is empty."}
			</div>
		);
	}

	return (
		<div
			aria-label={label}
			className={cn(
				"min-h-0 flex-1 overflow-y-auto overscroll-contain",
				className,
			)}
			onKeyDown={onKeyDown}
			ref={scrollRef}
			role="list"
		>
			<div
				className="relative w-full"
				style={{ height: virtualizer.getTotalSize() }}
			>
				{virtualizer.getVirtualItems().map((item) => {
					const isParent = parentLink !== undefined && item.index === 0;
					const entry = isParent ? null : entries[item.index - offset];
					const link = isParent ? parentLink : entry ? linkFor(entry) : null;
					if (!link) return null;
					const Icon = entry ? entryIcon(entry) : CornerLeftUpIcon;
					const selected = item.index === selectedIndex;
					return (
						<div
							className="absolute top-0 left-0 w-full px-1.5"
							key={entry?.path ?? ".."}
							role="listitem"
							style={{
								height: rowHeight,
								transform: `translateY(${item.start}px)`,
							}}
						>
							<Link
								{...link}
								aria-current={selected ? "true" : undefined}
								className={cn(
									"flex h-full items-center gap-2 rounded-md px-2 text-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
									selected && "bg-accent font-medium text-accent-foreground",
								)}
								data-row={item.index}
								onFocus={() => setFocusIndex(item.index)}
								tabIndex={item.index === focusIndex ? 0 : -1}
							>
								<Icon
									className={cn(
										"size-4 shrink-0",
										entry?.kind === "dir"
											? "fill-current/10 text-muted-foreground"
											: "text-muted-foreground/80",
									)}
								/>
								<span className="min-w-0 flex-1 truncate" title={entry?.name}>
									{entry ? entry.name : ".."}
								</span>
								{entry && (
									<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
										{entry.kind === "dir"
											? entry.children !== undefined
												? formatNumber(entry.children)
												: ""
											: formatBytes(entry.size)}
									</span>
								)}
							</Link>
						</div>
					);
				})}
			</div>
		</div>
	);
});
