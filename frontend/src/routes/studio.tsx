import { ClientOnly, createFileRoute } from "@tanstack/react-router";
import * as React from "react";
import { z } from "zod";
import { LogoMark } from "@/components/shell/logo";
import { Spinner } from "@/components/ui/spinner";
import { pageTitle } from "@/lib/route-meta";

/** The Studio bundle (three, r3f, drei) loads only when the Studio opens. */
const loadStudio = () => import("@/components/studio/studio-app");
const StudioApp = React.lazy(loadStudio);

const searchSchema = z.object({
	bg: z.string().max(32).optional().catch(undefined),
	clip: z.string().max(200).optional().catch(undefined),
});

export const Route = createFileRoute("/studio")({
	validateSearch: searchSchema,
	// Start fetching the 3D chunk while the route is matched / preloaded.
	loader: () => {
		if (typeof window !== "undefined") void loadStudio();
	},
	head: () => ({ meta: [{ title: pageTitle("Studio") }] }),
	staticData: { crumb: "Studio", fullBleed: true },
	component: StudioRoute,
});

function StudioSplash(): React.ReactElement {
	return (
		<main className="flex h-dvh w-screen items-center justify-center bg-[radial-gradient(120%_90%_at_50%_38%,#ffffff_0%,#ebeaf2_45%,#c4c2d3_100%)] dark:bg-[radial-gradient(120%_90%_at_50%_38%,#353048_0%,#1a1824_50%,#09080d_100%)]">
			<div className="flex flex-col items-center gap-3 text-muted-foreground text-sm">
				<LogoMark className="size-8" />
				<span className="flex items-center gap-2">
					<Spinner className="size-4" />
					Preparing the Studio…
				</span>
			</div>
		</main>
	);
}

function StudioRoute(): React.ReactElement {
	return (
		<ClientOnly fallback={<StudioSplash />}>
			<React.Suspense fallback={<StudioSplash />}>
				<StudioApp />
			</React.Suspense>
		</ClientOnly>
	);
}
