import { Link } from "@tanstack/react-router";
import {
	ChevronLeftIcon,
	ChevronRightIcon,
	CopyIcon,
	FolderOpenIcon,
} from "lucide-react";
import * as React from "react";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogDescription,
	DialogPopup,
	DialogTitle,
} from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { ImageViewer } from "@/components/viewers/image-viewer";
import { fileUrl, thumbUrl } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import { dirname, formatNumber } from "@/lib/format";
import type { GridItem } from "./texture-grid";

/** Full-size texture viewer with ←/→ navigation through the current results. */
export function Lightbox({
	items,
	index,
	onIndexChange,
	onClose,
	total,
}: {
	items: GridItem[];
	/** Total result count when `items` is only the loaded part. */
	total?: number;
	index: number | null;
	onIndexChange: (index: number) => void;
	onClose: () => void;
}): React.ReactElement {
	const item = index === null ? undefined : items[index];
	const open = item !== undefined;

	const go = React.useCallback(
		(delta: number) => {
			if (index === null || items.length === 0) return;
			onIndexChange((index + delta + items.length) % items.length);
		},
		[index, items.length, onIndexChange],
	);

	// Prefetch neighbours so stepping through feels instant.
	React.useEffect(() => {
		if (index === null) return;
		for (const delta of [1, -1]) {
			const next = items[(index + delta + items.length) % items.length];
			if (next) {
				const img = new Image();
				img.src = fileUrl("textures", next.path);
			}
		}
	}, [index, items]);

	return (
		<Dialog
			onOpenChange={(next) => {
				if (!next) onClose();
			}}
			open={open}
		>
			<DialogPopup
				bottomStickOnMobile={false}
				className="h-[min(92dvh,1100px)] max-w-[min(96vw,1600px)] overflow-hidden"
				onKeyDown={(event) => {
					if (event.key === "ArrowRight") {
						event.preventDefault();
						go(1);
					} else if (event.key === "ArrowLeft") {
						event.preventDefault();
						go(-1);
					}
				}}
			>
				{item && (
					<>
						<div className="flex shrink-0 items-center gap-3 border-b py-2 ps-4 pe-12">
							<img
								alt={item.name}
								className="size-8 shrink-0 rounded border bg-checker object-contain"
								src={thumbUrl(item.path, 128)}
							/>
							<div className="min-w-0 flex-1">
								<DialogTitle className="truncate font-medium text-sm">
									{item.name}
								</DialogTitle>
								<div className="flex min-w-0 items-center gap-1">
									<DialogDescription
										className="truncate font-mono text-xs"
										title={item.path}
									>
										{item.path}
									</DialogDescription>
									<Button
										aria-label="Copy path"
										className="shrink-0"
										onClick={() => void copyText(item.path, "Path")}
										size="icon-xs"
										title="Copy path"
										variant="ghost"
									>
										<CopyIcon />
									</Button>
								</div>
							</div>
							{items.length > 1 && (
								<span className="hidden shrink-0 items-center gap-1 text-muted-foreground text-xs md:flex">
									<Kbd>←</Kbd>
									<Kbd>→</Kbd>
									<span>step</span>
								</span>
							)}
							<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
								{(index ?? 0) + 1} /{" "}
								{formatNumber(Math.max(total ?? 0, items.length))}
							</span>
							<Button
								render={
									<Link
										params={{ root: "textures" }}
										search={{ path: dirname(item.path), file: item.path }}
										to="/browse/$root"
									/>
								}
								size="sm"
								variant="outline"
							>
								<FolderOpenIcon />
								<span className="max-sm:hidden">Open in browser</span>
							</Button>
						</div>
						<div className="relative flex min-h-0 flex-1">
							<ImageViewer
								alt={item.name}
								downloadName={item.name}
								key={item.path}
								src={fileUrl("textures", item.path)}
							/>
							{items.length > 1 && (
								<>
									<Button
										aria-label="Previous texture"
										className="absolute top-1/2 left-3 -translate-y-1/2 rounded-full shadow-md"
										onClick={() => go(-1)}
										size="icon"
										variant="outline"
									>
										<ChevronLeftIcon />
									</Button>
									<Button
										aria-label="Next texture"
										className="absolute top-1/2 right-3 -translate-y-1/2 rounded-full shadow-md"
										onClick={() => go(1)}
										size="icon"
										variant="outline"
									>
										<ChevronRightIcon />
									</Button>
								</>
							)}
						</div>
					</>
				)}
			</DialogPopup>
		</Dialog>
	);
}
