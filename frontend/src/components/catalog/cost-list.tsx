import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import * as React from "react";
import { EntityImage } from "@/components/catalog/entity-image";
import { rarityStyle } from "@/components/catalog/rarity";
import { type Cost, catalogListQuery, type ListItem } from "@/lib/catalog";
import { formatNumber } from "@/lib/format";
import { useSettings } from "@/lib/settings";
import { cn } from "@/lib/utils";

/** id → list item for the items catalog (names + icons for cost chips). */
export function useItemIndex(): Map<string, ListItem> | undefined {
	const { textLanguage } = useSettings();
	const { data } = useQuery(catalogListQuery("items", textLanguage));
	return React.useMemo(() => {
		if (!data) return undefined;
		return new Map(data.items.map((item) => [item.id, item]));
	}, [data]);
}

function formatCount(n: number): string {
	if (n >= 10_000) return `${+(n / 1000).toFixed(1)}k`;
	return formatNumber(n);
}

/** Material costs as item chips linking to the item pages. */
export function CostList({
	costs,
	gold,
	className,
}: {
	costs: Cost[];
	/** Extra T-Creds cost given separately (weapon breakthroughs). */
	gold?: number;
	className?: string;
}): React.ReactElement | null {
	const index = useItemIndex();
	const all = React.useMemo(() => {
		const out = costs.filter((c) => c.count > 0);
		if (gold) out.push({ id: "item_gold", count: gold });
		return out;
	}, [costs, gold]);
	if (all.length === 0) return null;
	return (
		<ul className={cn("flex flex-wrap gap-1.5", className)}>
			{all.map((cost) => {
				const item = index?.get(cost.id);
				return (
					<li key={cost.id}>
						<Link
							className="group/cost inline-flex h-8 items-center gap-1.5 rounded-lg border bg-background py-0.5 ps-0.5 pe-2 text-xs outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring dark:bg-input/32"
							params={{ kind: "items", id: cost.id }}
							title={item?.name ?? cost.id}
							to="/catalog/$kind/$id"
						>
							<span
								className="relative size-7 shrink-0 overflow-hidden rounded-md bg-muted"
								style={rarityStyle(item?.rarity)}
							>
								<EntityImage
									className="size-full text-[0.5rem]"
									name={item?.name ?? "?"}
									path={item?.icon}
									size={128}
								/>
								{item?.rarity ? (
									<span className="absolute inset-x-0 bottom-0 h-0.5 bg-(--rarity)" />
								) : null}
							</span>
							<span className="max-w-36 truncate text-foreground/80 group-hover/cost:text-foreground">
								{item?.name ?? cost.id}
							</span>
							<span className="font-medium tabular-nums">
								×{formatCount(cost.count)}
							</span>
						</Link>
					</li>
				);
			})}
		</ul>
	);
}
