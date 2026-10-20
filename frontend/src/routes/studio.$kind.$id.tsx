import { createFileRoute } from "@tanstack/react-router";
import { pageTitle } from "@/lib/route-meta";

/** `/studio/$kind/$id`: the parent route renders the Studio for these params. */
export const Route = createFileRoute("/studio/$kind/$id")({
	head: ({ params }) => ({ meta: [{ title: pageTitle(params.id, "Studio") }] }),
	staticData: { crumb: ({ id }) => id ?? "Model" },
	component: () => null,
});
