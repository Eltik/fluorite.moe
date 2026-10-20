import * as React from "react";
import { EntityImage } from "@/components/catalog/entity-image";
import { Lightbox } from "@/components/gallery/lightbox";
import type { GridItem } from "@/components/gallery/texture-grid";

const ART_LABELS: Record<string, string> = {
	portrait: "Portrait",
	gacha: "Recruitment art",
	gachaShadow: "Recruitment silhouette",
	icon: "Icon",
	iconBig: "Large icon",
	roundIcon: "Round icon",
	horizontalHeadIcon: "Banner head",
	remoteIcon: "Remote icon",
	battleIcon: "Battle icon",
	infoBackground: "Info background",
	adventureIcon: "Adventure icon",
	image: "Illustration",
};

export function artLabel(key: string): string {
	return ART_LABELS[key] ?? key.replace(/([a-z])([A-Z])/g, "$1 $2");
}

/** Every art path of an entity as a thumbnail grid that opens the lightbox. */
export function ArtGallery({
	art,
	name,
}: {
	art: Record<string, string | null | undefined>;
	name: string;
}): React.ReactElement {
	const items = React.useMemo<GridItem[]>(
		() =>
			Object.entries(art)
				.filter((entry): entry is [string, string] => Boolean(entry[1]))
				.map(([key, path]) => ({ path, name: `${name} · ${artLabel(key)}` })),
		[art, name],
	);
	const [open, setOpen] = React.useState<number | null>(null);

	return (
		<>
			<ul className="grid grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] gap-3">
				{items.map((item, i) => (
					<li key={item.path}>
						<button
							className="group/art flex w-full cursor-zoom-in flex-col overflow-hidden rounded-xl border bg-card text-start shadow-xs/5 outline-none transition-shadow hover:shadow-md/5 focus-visible:ring-2 focus-visible:ring-ring"
							onClick={() => setOpen(i)}
							type="button"
						>
							<EntityImage
								className="aspect-square w-full bg-checker p-2"
								imgClassName="transition-transform duration-200 group-hover/art:scale-[1.03] motion-reduce:transform-none"
								path={item.path}
								size={256}
							/>
							<span className="truncate border-t px-2.5 py-2 text-xs">
								{item.name.split(" · ").pop()}
							</span>
						</button>
					</li>
				))}
			</ul>
			<Lightbox
				index={open}
				items={items}
				onClose={() => setOpen(null)}
				onIndexChange={setOpen}
			/>
		</>
	);
}
