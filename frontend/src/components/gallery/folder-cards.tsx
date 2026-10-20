import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { FolderIcon } from "lucide-react";
import * as React from "react";
import { Button } from "@/components/ui/button";
import { api, thumbUrl } from "@/lib/api";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

const PREVIEW = 18;

export interface FolderEntry {
	path: string;
	name: string;
	children?: number;
}

/** First few PNGs anywhere under `path` (search matches path substrings). */
const folderSampleQuery = (path: string) => ({
	queryKey: ["texture-folder-sample", path],
	queryFn: ({ signal }: { signal: AbortSignal }) =>
		api.search(
			{ q: `${path}/`, root: "textures", ext: "png", limit: 4 },
			signal,
		),
	staleTime: 10 * 60_000,
});

/** Shows a card only once it scrolls near the viewport. */
function useNearViewport<T extends Element>(): [
	React.RefObject<T | null>,
	boolean,
] {
	const ref = React.useRef<T>(null);
	const [seen, setSeen] = React.useState(false);
	React.useEffect(() => {
		const el = ref.current;
		if (!el || seen) return;
		const io = new IntersectionObserver(
			(entries) => {
				if (entries.some((e) => e.isIntersecting)) setSeen(true);
			},
			{ rootMargin: "200px" },
		);
		io.observe(el);
		return () => io.disconnect();
	}, [seen]);
	return [ref, seen];
}

function FolderCard({ folder }: { folder: FolderEntry }): React.ReactElement {
	const [ref, visible] = useNearViewport<HTMLAnchorElement>();
	const sample = useQuery({
		...folderSampleQuery(folder.path),
		enabled: visible,
	});
	const thumbs = (sample.data?.items ?? []).map((it) => it.path);
	return (
		<Link
			className="group flex flex-col gap-1.5 rounded-lg outline-none"
			ref={ref}
			search={{ dir: folder.path }}
			title={folder.path}
			to="/textures"
		>
			<span className="grid aspect-[4/3] grid-cols-2 grid-rows-2 gap-px overflow-hidden rounded-lg border bg-checker transition-shadow group-hover:ring-2 group-hover:ring-ring/40 group-focus-visible:ring-2 group-focus-visible:ring-ring">
				{[0, 1, 2, 3].map((i) => {
					const path = thumbs[i];
					return (
						<span
							className={cn(
								"flex items-center justify-center overflow-hidden bg-card/40",
								sample.isPending && visible && "animate-pulse bg-muted",
							)}
							key={i}
						>
							{path ? (
								<img
									alt=""
									className="max-h-full max-w-full object-contain p-1"
									decoding="async"
									draggable={false}
									loading="lazy"
									src={thumbUrl(path, 128)}
								/>
							) : i === 0 && !sample.isPending ? (
								<FolderIcon className="size-5 text-muted-foreground" />
							) : null}
						</span>
					);
				})}
			</span>
			<span className="flex min-w-0 items-baseline gap-1.5 px-0.5 text-sm">
				<FolderIcon className="size-3.5 shrink-0 self-center fill-current/10 text-muted-foreground" />
				<span className="truncate font-medium group-hover:text-foreground">
					{folder.name}
				</span>
				{folder.children !== undefined && (
					<span className="ms-auto shrink-0 text-muted-foreground text-xs tabular-nums">
						{formatNumber(folder.children)}
					</span>
				)}
			</span>
		</Link>
	);
}

/** Subfolders as cards with a 2×2 thumbnail mosaic of their contents. */
export function FolderCards({
	folders,
	compact,
}: {
	folders: FolderEntry[];
	/** Many folders above a populated grid: start collapsed. */
	compact?: boolean;
}): React.ReactElement {
	const [expanded, setExpanded] = React.useState(false);
	const limit = compact ? 6 : PREVIEW;
	const shown = expanded ? folders : folders.slice(0, limit);
	return (
		<section aria-label="Subfolders" className="flex flex-col gap-2">
			<ul className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-3">
				{shown.map((folder) => (
					<li key={folder.path}>
						<FolderCard folder={folder} />
					</li>
				))}
			</ul>
			{folders.length > limit && (
				<Button
					className="self-start"
					onClick={() => setExpanded((v) => !v)}
					size="sm"
					variant="ghost"
				>
					{expanded
						? "Show fewer folders"
						: `Show all ${formatNumber(folders.length)} folders`}
				</Button>
			)}
		</section>
	);
}
