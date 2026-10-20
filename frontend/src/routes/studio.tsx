import { createFileRoute, Link } from "@tanstack/react-router";
import { BoxesIcon } from "lucide-react";
import type * as React from "react";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { pageTitle } from "@/lib/route-meta";

export const Route = createFileRoute("/studio")({
	head: () => ({ meta: [{ title: pageTitle("Studio") }] }),
	staticData: { crumb: "Studio" },
	component: Studio,
});

function Studio(): React.ReactElement {
	return (
		<div className="relative flex flex-1 flex-col overflow-hidden">
			{/* Soft stage glow hinting at the 3D viewport to come. */}
			<div
				aria-hidden="true"
				className="pointer-events-none absolute inset-x-0 top-1/3 mx-auto h-72 max-w-2xl rounded-full bg-violet-500/10 blur-3xl dark:bg-violet-400/10"
			/>
			<Empty className="relative">
				<EmptyHeader>
					<EmptyMedia variant="icon">
						<BoxesIcon />
					</EmptyMedia>
					<EmptyTitle>The studio is being built</EmptyTitle>
					<EmptyDescription>
						A full 3D viewer for operators, enemies and weapons, with
						animations, in-game shading and props, is on its way. Until then,
						static models can be previewed from the model browser.
					</EmptyDescription>
				</EmptyHeader>
				<EmptyContent className="flex-row justify-center gap-2">
					<Button
						render={<Link params={{ root: "meshes" }} to="/browse/$root" />}
					>
						Browse models
					</Button>
					<Button render={<Link to="/" />} variant="outline">
						Overview
					</Button>
				</EmptyContent>
			</Empty>
		</div>
	);
}
