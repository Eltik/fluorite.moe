import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import * as React from "react";
import { z } from "zod";
import type { DetailProps } from "@/components/catalog/detail-layout";
import { TermsProvider } from "@/components/rich-text/rich-text";
import { DefaultPending, ErrorState } from "@/components/status-views";
import { type CatalogKind, isApiError } from "@/lib/api";
import {
	type EntityDetail,
	entityQuery,
	type Gender,
	KIND_LABELS,
	loaderLanguage,
	searchText,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

// Each kind's body is its own chunk; only the one being viewed is loaded.
// biome-ignore lint/suspicious/noExplicitAny: bodies are typed per kind below
type Body = React.ComponentType<DetailProps<any>>;
const BODIES: Record<CatalogKind, React.LazyExoticComponent<Body>> = {
	operators: React.lazy(() => import("@/components/catalog/operator-detail")),
	weapons: React.lazy(() => import("@/components/catalog/weapon-detail")),
	enemies: React.lazy(() => import("@/components/catalog/enemy-detail")),
	items: React.lazy(() =>
		import("@/components/catalog/misc-detail").then((m) => ({
			default: m.ItemDetail,
		})),
	),
	npcs: React.lazy(() =>
		import("@/components/catalog/misc-detail").then((m) => ({
			default: m.NpcDetail,
		})),
	),
	facilities: React.lazy(() =>
		import("@/components/catalog/misc-detail").then((m) => ({
			default: m.FacilityDetail,
		})),
	),
};

const searchSchema = z.object({
	tab: searchText,
	gender: z.enum(["f", "m"]).optional().catch(undefined),
});

export const Route = createFileRoute("/catalog/$kind/$id")({
	validateSearch: searchSchema,
	loaderDeps: ({ search }) => ({ gender: (search.gender ?? "f") as Gender }),
	loader: async ({ context, params, deps }) => {
		const kind = params.kind as CatalogKind;
		try {
			const data = await context.queryClient.ensureQueryData(
				entityQuery(kind, params.id, loaderLanguage(), deps.gender),
			);
			return { name: data.name };
		} catch (error) {
			if (isApiError(error) && error.isNotFound) throw notFound();
			throw error;
		}
	},
	head: ({ params, loaderData }) => ({
		meta: [
			{
				title: pageTitle(
					loaderData?.name ?? params.id,
					KIND_LABELS[params.kind as CatalogKind] ?? "Catalog",
				),
			},
		],
	}),
	staticData: {
		crumb: ({ id }, loaderData) =>
			(loaderData as { name?: string } | undefined)?.name ?? id ?? "",
	},
	component: EntityPage,
});

function EntityPage(): React.ReactElement {
	const { kind: rawKind, id } = Route.useParams();
	const kind = rawKind as CatalogKind;
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const gender: Gender = search.gender ?? "f";
	const query = useQuery({
		...entityQuery(kind, id, textLanguage, gender),
		// Keep the current page while another language / gender loads.
		placeholderData: (prev, prevQuery) =>
			prevQuery?.queryKey[1] === kind && prevQuery.queryKey[4] === id
				? prev
				: undefined,
	});

	const onTabChange = React.useCallback(
		(tab: string) =>
			void navigate({
				search: (prev) => ({ ...prev, tab }),
				replace: true,
				resetScroll: false,
			}),
		[navigate],
	);
	const onGenderChange = React.useCallback(
		(next: Gender) =>
			void navigate({
				search: (prev) => ({
					...prev,
					gender: next === "f" ? undefined : next,
				}),
				replace: true,
				resetScroll: false,
			}),
		[navigate],
	);

	if (!query.data) {
		if (query.isError) {
			return (
				<ErrorState
					className="min-h-[60vh]"
					error={query.error}
					onRetry={() => void query.refetch()}
				/>
			);
		}
		return <DefaultPending />;
	}
	const data = query.data as EntityDetail;
	const Body = BODIES[kind];
	return (
		<TermsProvider terms={data.terms}>
			<React.Suspense fallback={<DefaultPending />}>
				<Body
					data={data}
					key={`${kind}/${id}`}
					gender={gender}
					onGenderChange={onGenderChange}
					onTabChange={onTabChange}
					tab={search.tab}
				/>
			</React.Suspense>
		</TermsProvider>
	);
}
