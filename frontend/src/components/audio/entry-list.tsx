import { useVirtualizer } from "@tanstack/react-virtual";
import { PackageIcon } from "lucide-react";
import * as React from "react";
import { Badge } from "@/components/ui/badge";
import { type AudioEntry, formatDuration } from "@/lib/audio";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { AudioButton } from "./audio-button";
import { useAudioState } from "./engine";

const ROW = 52;

/** Track key of an entry (its preferred clip URL), shared with `AudioButton`. */
export function entryKey(e: AudioEntry): string {
	return e.audio?.ogg ?? e.audio?.m4a ?? `${e.pack}/${e.id}`;
}

export function entryTitle(e: AudioEntry): string {
	return e.name ?? e.events?.find((ev) => ev.name)?.name ?? `#${e.id}`;
}

/** Voice path, or pack plus the other events that play this media. */
export function entrySubtitle(e: AudioEntry): string {
	if (e.path) return e.path;
	const title = entryTitle(e);
	const events = e.events ?? [];
	const others = events
		.map((ev) => ev.name)
		.filter((n): n is string => Boolean(n) && n !== title);
	const parts = [e.pack];
	if (others.length) parts.push(others.slice(0, 2).join(", "));
	else if (events.length && !events.some((ev) => ev.name))
		parts.push(`${events.length} unnamed event${events.length > 1 ? "s" : ""}`);
	if (title !== `#${e.id}`) parts.push(`#${e.id}`);
	return parts.join(" · ");
}

/**
 * Virtualized entry list. Rows play on click; `selected` is the keyboard
 * cursor, kept in view. `onNearEnd` fires when the last rows render so the
 * caller can fetch the next page.
 */
export function AudioEntryList({
	entries,
	total,
	selected,
	onSelect,
	onNearEnd,
	className,
}: {
	entries: AudioEntry[];
	total: number;
	selected: number;
	onSelect: (index: number, play: boolean) => void;
	onNearEnd?: () => void;
	className?: string;
}): React.ReactElement {
	const scrollRef = React.useRef<HTMLDivElement>(null);
	const current = useAudioState().track?.key;
	const virtualizer = useVirtualizer({
		count: entries.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => ROW,
		overscan: 10,
	});
	const items = virtualizer.getVirtualItems();
	const last = items.at(-1)?.index ?? 0;

	React.useEffect(() => {
		if (entries.length && last >= entries.length - 20 && entries.length < total)
			onNearEnd?.();
	}, [last, entries.length, total, onNearEnd]);

	React.useEffect(() => {
		if (selected >= 0 && selected < entries.length)
			virtualizer.scrollToIndex(selected, { align: "auto" });
	}, [selected, entries.length, virtualizer]);

	return (
		<div
			aria-label="Audio entries"
			className={cn("min-h-0 overflow-y-auto", className)}
			ref={scrollRef}
			role="listbox"
			tabIndex={-1}
		>
			<div
				className="relative w-full"
				style={{ height: virtualizer.getTotalSize() }}
			>
				{items.map((item) => {
					const e = entries[item.index];
					if (!e) return null;
					const key = entryKey(e);
					const isCurrent = current === key;
					const isSelected = item.index === selected;
					return (
						<div
							aria-selected={isSelected}
							className={cn(
								"absolute inset-x-0 flex cursor-pointer items-center gap-3 border-b px-2 transition-colors hover:bg-accent/50 sm:px-3",
								isSelected && "bg-accent/70 hover:bg-accent/70",
								isCurrent && "text-primary",
							)}
							data-index={item.index}
							key={`${e.pack}/${e.id}`}
							onClick={() => onSelect(item.index, true)}
							onKeyDown={(event) => {
								if (event.key === "Enter") onSelect(item.index, true);
							}}
							role="option"
							style={{
								height: item.size,
								transform: `translateY(${item.start}px)`,
							}}
							tabIndex={-1}
						>
							{e.audio ? (
								<AudioButton
									src={e.audio}
									subtitle={entrySubtitle(e)}
									title={entryTitle(e)}
								/>
							) : (
								<span className="flex size-8 shrink-0 items-center justify-center text-muted-foreground sm:size-7">
									<PackageIcon className="size-4" />
								</span>
							)}
							<div className="min-w-0 flex-1">
								<div className="truncate font-medium text-sm">
									{entryTitle(e)}
								</div>
								<div className="truncate text-muted-foreground text-xs">
									{entrySubtitle(e)}
								</div>
							</div>
							<div className="flex shrink-0 items-center gap-2 text-muted-foreground text-xs tabular-nums">
								<Badge className="w-9" size="sm" variant="outline">
									{e.lang}
								</Badge>
								<span className="hidden w-12 sm:inline">{e.codec ?? "—"}</span>
								<span className="w-10 text-right font-mono">
									{e.kind === "bank"
										? "bank"
										: formatDuration(
												e.duration === undefined
													? undefined
													: Math.max(1, Math.round(e.duration)),
											)}
								</span>
								<span className="hidden w-16 text-right md:inline">
									{formatBytes(e.size)}
								</span>
							</div>
						</div>
					);
				})}
			</div>
		</div>
	);
}
