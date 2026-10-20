import { createFileRoute, notFound, Outlet } from "@tanstack/react-router";
import { isCatalogKind, KIND_LABELS } from "@/lib/catalog";

/** Layout for one catalog kind: validates the kind and owns its crumb. */
export const Route = createFileRoute("/catalog/$kind")({
	beforeLoad: ({ params }) => {
		if (!isCatalogKind(params.kind)) throw notFound();
	},
	staticData: {
		section: "Catalog",
		crumb: ({ kind }) =>
			kind && isCatalogKind(kind) ? KIND_LABELS[kind] : "Catalog",
	},
	component: Outlet,
});
