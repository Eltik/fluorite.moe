import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import * as React from "react";
import { AudioCard, StoryCard } from "@/components/home/entry-cards";
import { FeaturedHero } from "@/components/home/featured-hero";
import { byRarity, OperatorStrip } from "@/components/home/operator-strip";
import {
	CatalogLinks,
	GameFiles,
	RecentlyViewed,
} from "@/components/home/sections";
import type { ListItem } from "@/lib/catalog";
import { catalogListQuery, loaderLanguage } from "@/lib/catalog";
import { catalogKindsQuery, statusQuery } from "@/lib/queries";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

/** Operators worth a hero: high rarity with a Studio model. */
function featurable(items: ListItem[]): ListItem[] {
	return items.filter((item) => (item.rarity ?? 0) >= 5 && item.hasModel);
}

export const Route = createFileRoute("/")({
	loader: async ({ context }) => {
		const { queryClient } = context;
		// Failures are non-blocking: each section renders its own offline state.
		const [list] = await Promise.all([
			queryClient
				.ensureQueryData(catalogListQuery("operators", loaderLanguage()))
				.catch(() => undefined),
			queryClient.ensureQueryData(statusQuery()).catch(() => undefined),
			queryClient.ensureQueryData(catalogKindsQuery()).catch(() => undefined),
		]);
		// Picked here (not during render) so server and client agree: the
		// loader result is serialized into the page and reused on hydration.
		const seed = Math.floor(Math.random() * 1_000_000);
		const pool = list ? featurable(list.items) : [];
		return { featured: pool[seed % (pool.length || 1)]?.id ?? null, seed };
	},
	head: () => ({ meta: [{ title: pageTitle("Home") }] }),
	staticData: { crumb: "Home" },
	component: Home,
});

function Home(): React.ReactElement {
	const loaderData = Route.useLoaderData();
	const { textLanguage } = useSettings();
	const operators = useQuery({
		...catalogListQuery("operators", textLanguage),
		placeholderData: (prev) => prev,
	});
	const [featuredId, setFeaturedId] = React.useState(loaderData.featured);
	React.useEffect(() => setFeaturedId(loaderData.featured), [loaderData]);

	const items = React.useMemo(
		() => (operators.data ? byRarity(operators.data.items) : undefined),
		[operators.data],
	);
	const featured = items?.find((item) => item.id === featuredId);
	const pool = items ? featurable(items) : [];

	const shuffle =
		pool.length > 1
			? () => {
					const others = pool.filter((item) => item.id !== featuredId);
					const next = others[Math.floor(Math.random() * others.length)];
					if (next) setFeaturedId(next.id);
				}
			: undefined;

	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-10 p-4 md:p-6 lg:p-8">
			<h1 className="sr-only">fluorite: an Arknights: Endfield data studio</h1>
			<FeaturedHero item={featured} onShuffle={shuffle} />
			<OperatorStrip items={items} total={operators.data?.total} />
			<div className="grid gap-4 md:grid-cols-2">
				<StoryCard lang={textLanguage} seed={loaderData.seed} />
				<AudioCard
					lang={textLanguage}
					operatorId={featuredId ?? undefined}
					seed={loaderData.seed}
				/>
			</div>
			<CatalogLinks />
			<RecentlyViewed />
			<GameFiles />
		</div>
	);
}
