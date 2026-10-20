import { Link } from "@tanstack/react-router";
import { ArrowRightIcon, BoxesIcon } from "lucide-react";
import type * as React from "react";
import { Skeleton } from "@/components/ui/skeleton";
import { thumbUrl } from "@/lib/api";
import { type ListItem, rarityColor } from "@/lib/catalog";
import { studioLink } from "@/lib/studio";

/** The two Endministrator variants share a name; tell them apart. */
function variantMark(id: string): string | null {
	if (id.endsWith("endminm")) return "♂";
	if (id.endsWith("endminf")) return "♀";
	return null;
}

/** Highest rarity first; the API's game order breaks ties. */
export function byRarity(items: ListItem[]): ListItem[] {
	return items
		.map((item, index) => ({ item, index }))
		.sort(
			(a, b) =>
				(b.item.rarity ?? 0) - (a.item.rarity ?? 0) || a.index - b.index,
		)
		.map((e) => e.item);
}

export function OperatorStrip({
	items,
	total,
}: {
	items: ListItem[] | undefined;
	total?: number;
}): React.ReactElement {
	return (
		<section
			aria-labelledby="operators-heading"
			className="flex flex-col gap-3"
		>
			<div className="flex items-end justify-between gap-4">
				<div>
					<h2
						className="font-semibold text-lg tracking-tight"
						id="operators-heading"
					>
						Operators
					</h2>
					<p className="text-muted-foreground text-sm">
						{total ? `${total} playable operators, ` : ""}rarest first. Every
						one has a model in the Studio.
					</p>
				</div>
				<Link
					className="flex shrink-0 items-center gap-1 rounded-md font-medium text-sm hover:underline"
					params={{ kind: "operators" }}
					to="/catalog/$kind"
				>
					All operators
					<ArrowRightIcon aria-hidden="true" className="size-4" />
				</Link>
			</div>
			<div className="-mx-4 md:-mx-6 lg:-mx-8 overflow-x-auto overscroll-x-contain [scrollbar-width:thin]">
				<ul className="flex w-max gap-3 px-4 pt-1 pb-3 md:px-6 lg:px-8">
					{items
						? items.map((item) => <OperatorCard item={item} key={item.id} />)
						: Array.from({ length: 10 }, (_, i) => (
								// biome-ignore lint/suspicious/noArrayIndexKey: static skeletons
								<li className="flex w-24 flex-col gap-2" key={i}>
									<Skeleton className="size-24 rounded-xl" />
									<Skeleton className="h-4 w-16" />
								</li>
							))}
				</ul>
			</div>
		</section>
	);
}

function OperatorCard({ item }: { item: ListItem }): React.ReactElement {
	const color = rarityColor(item.rarity) ?? "var(--border)";
	const mark = variantMark(item.id);
	return (
		<li className="group/op relative w-24 shrink-0">
			<Link
				className="flex flex-col gap-1.5 rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
				params={{ kind: "operators", id: item.id }}
				title={item.name}
				to="/catalog/$kind/$id"
			>
				<span
					className="relative block size-24 overflow-hidden rounded-xl border bg-muted transition-transform duration-200 group-hover/op:-translate-y-0.5"
					style={{
						backgroundImage: `linear-gradient(to top, color-mix(in oklab, ${color} 35%, transparent), transparent 60%)`,
					}}
				>
					{item.icon && (
						<img
							alt=""
							className="size-full object-cover"
							decoding="async"
							height={96}
							loading="lazy"
							src={thumbUrl(item.icon, 128)}
							width={96}
						/>
					)}
					<span
						aria-hidden="true"
						className="absolute inset-x-0 bottom-0 h-1"
						style={{ backgroundColor: color }}
					/>
					{mark && (
						<span
							aria-hidden="true"
							className="absolute top-1 left-1 flex size-5 items-center justify-center rounded-full bg-background/80 text-xs backdrop-blur-sm"
						>
							{mark}
						</span>
					)}
				</span>
				<span className="truncate px-0.5 font-medium text-xs">
					{item.name}
					{mark && (
						<span className="sr-only">
							{mark === "♂" ? " (male)" : " (female)"}
						</span>
					)}
				</span>
			</Link>
			{item.hasModel && (
				<Link
					{...studioLink("operators", item.id)}
					aria-label={`Open ${item.name} in Studio`}
					className="absolute top-1 right-1 flex size-7 items-center justify-center rounded-lg bg-background/85 text-foreground opacity-0 shadow-sm backdrop-blur-sm transition-opacity hover:bg-background focus-visible:opacity-100 group-hover/op:opacity-100 pointer-coarse:opacity-100"
					title="Open in Studio"
				>
					<BoxesIcon aria-hidden="true" className="size-3.5" />
				</Link>
			)}
		</li>
	);
}
