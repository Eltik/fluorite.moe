import { createFileRoute } from "@tanstack/react-router";

/** `/studio` opens with the model picker; the parent route renders the Studio. */
export const Route = createFileRoute("/studio/")({
	component: () => null,
});
