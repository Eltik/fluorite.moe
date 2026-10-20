import { ClientOnly, createFileRoute } from "@tanstack/react-router";
import * as React from "react";
import { LogoMark } from "@/components/shell/logo";
import { Spinner } from "@/components/ui/spinner";
import { pageTitle } from "@/lib/route-meta";
import { searchParams, str } from "@/lib/search-params";

/** The Studio bundle (three, r3f, drei) loads only when the Studio opens. */
const loadStudio = () => import("@/components/studio/studio-app");
const StudioApp = React.lazy(loadStudio);

const parseSearch = searchParams({
	bg: str(32),
	clip: str(200),
});

export const Route = createFileRoute("/studio")({
	validateSearch: parseSearch,
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
