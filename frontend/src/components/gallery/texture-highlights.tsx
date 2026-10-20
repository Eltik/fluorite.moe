import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowRightIcon } from "lucide-react";
import type * as React from "react";
import { Skeleton } from "@/components/ui/skeleton";
import { api, thumbUrl } from "@/lib/api";
import { formatNumber } from "@/lib/format";

const SPRITES = "beyond/dynamicassets/gameplay/ui/sprites";

/** Hand-picked UI sprite folders that read well as a gallery. */
const HIGHLIGHTS: { title: string; dir: string }[] = [
	{ title: "Operator icons", dir: `${SPRITES}/charicon` },
	{ title: "Item icons", dir: `${SPRITES}/itemicon` },
	{ title: "Skill icons", dir: `${SPRITES}/skillicon` },
	{ title: "Enemy icons", dir: `${SPRITES}/monstericon` },
	{ title: "Medals", dir: `${SPRITES}/medalicon` },
	{ title: "Emoji", dir: `${SPRITES}/emoji` },
];

const SAMPLE = 10;

function HighlightRow({
	title,
	dir,
	onOpen,
}: {
	title: string;
	dir: string;
	onOpen: (path: string) => void;
}): React.ReactElement | null {
	const sample = useQuery({
		queryKey: ["tree", "textures", dir, "sample", SAMPLE],
		queryFn: ({ signal }) =>
			api.tree("textures", { path: dir, limit: SAMPLE + 4 }, signal),
		staleTime: 10 * 60_000,
	});
	if (sample.isError) return null;
	const files = (sample.data?.entries ?? [])
		.filter((e) => e.kind === "file" && e.name.endsWith(".png"))
		.slice(0, SAMPLE);
	return (
		<section className="flex flex-col gap-2">
			<div className="flex items-baseline gap-2">
				<h2 className="font-medium text-sm">{title}</h2>
				{sample.data && (
					<span className="text-muted-foreground text-xs tabular-nums">
						{formatNumber(sample.data.total)}
					</span>
				)}
				<Link
					className="ms-auto inline-flex items-center gap-1 rounded-sm text-muted-foreground text-xs outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
					search={{ dir }}
					to="/textures"
				>
					Open folder
					<ArrowRightIcon className="size-3" />
				</Link>
			</div>
			<ul className="grid grid-cols-[repeat(auto-fill,minmax(5.5rem,1fr))] gap-2">
				{sample.isPending
					? Array.from({ length: SAMPLE }, (_, i) => (
							// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
							<li key={i}>
								<Skeleton className="aspect-square rounded-lg" />
							</li>
						))
					: files.map((f) => (
							<li key={f.path}>
								<button
									aria-label={f.name}
									className="flex aspect-square w-full cursor-pointer items-center justify-center overflow-hidden rounded-lg border bg-checker outline-none transition-shadow hover:ring-2 hover:ring-ring/40 focus-visible:ring-2 focus-visible:ring-ring"
									onClick={() => onOpen(f.path)}
									title={f.path}
									type="button"
								>
									<img
										alt={f.name}
										className="max-h-full max-w-full object-contain"
										decoding="async"
										draggable={false}
										loading="lazy"
										src={thumbUrl(f.path, 128)}
									/>
								</button>
							</li>
						))}
			</ul>
		</section>
	);
}

/** Curated sample shown on folders that hold no textures themselves. */
export function TextureHighlights({
	onOpen,
}: {
	onOpen: (path: string) => void;
}): React.ReactElement {
	return (
		<div className="flex flex-col gap-6">
			<div>
				<h2 className="font-semibold text-base">Highlights</h2>
				<p className="text-muted-foreground text-sm">
					A few of the UI sprite folders. Browse the folders above for
					everything else.
				</p>
			</div>
			{HIGHLIGHTS.map((h) => (
				<HighlightRow dir={h.dir} key={h.dir} onOpen={onOpen} title={h.title} />
			))}
		</div>
	);
}
