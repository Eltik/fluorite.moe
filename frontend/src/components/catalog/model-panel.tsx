import { Link } from "@tanstack/react-router";
import { BoxesIcon, BoxIcon, ExternalLinkIcon } from "lucide-react";
import type * as React from "react";
import { Panel, Section } from "@/components/catalog/detail-layout";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import type { CatalogKind, CatalogModel } from "@/lib/api";
import { fileTarget } from "@/lib/nav";
import { isStudioKind, studioLink } from "@/lib/studio";

/** "Open in Studio" plus the static mesh parts, each linking to the file browser. */
export function ModelPanel({
	kind,
	id,
	model,
}: {
	kind: CatalogKind;
	id: string;
	model: CatalogModel | null | undefined;
}): React.ReactElement {
	const parts = model?.parts ?? [];
	if (!model || (parts.length === 0 && !model.rigged)) {
		return (
			<Empty className="rounded-2xl border border-dashed py-14">
				<EmptyHeader>
					<EmptyMedia variant="icon">
						<BoxIcon />
					</EmptyMedia>
					<EmptyTitle>No model found</EmptyTitle>
					<EmptyDescription>
						No meshes were exported for this entry.
					</EmptyDescription>
				</EmptyHeader>
			</Empty>
		);
	}
	return (
		<div className="flex flex-col gap-5">
			<Panel className="flex flex-wrap items-center justify-between gap-4">
				<div className="flex min-w-0 items-center gap-3">
					<div className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-muted text-muted-foreground">
						<BoxesIcon className="size-5" />
					</div>
					<div className="min-w-0">
						<div className="font-medium text-sm">
							{model.rigged ? "Rigged, textured model" : "Static mesh parts"}
						</div>
						<div
							className="truncate font-mono text-muted-foreground text-xs"
							title={model.dir}
						>
							{model.rigged ?? model.dir}
						</div>
					</div>
				</div>
				{isStudioKind(kind) && (
					<Button render={<Link {...studioLink(kind, id)} />}>
						<BoxesIcon />
						Open in Studio
					</Button>
				)}
			</Panel>
			{parts.length > 0 && (
				<Section
					title="Parts"
					description={`${parts.length} mesh parts, best LOD per part`}
				>
					<ul className="divide-y overflow-hidden rounded-xl border bg-card">
						{parts.map((part) => {
							const target = fileTarget("meshes", part.path);
							return (
								<li
									className="flex items-center gap-3 px-3 py-2"
									key={part.path}
								>
									<BoxIcon className="size-4 shrink-0 text-muted-foreground" />
									<span className="min-w-0 flex-1">
										<span className="block truncate font-medium text-sm">
											{part.name}
										</span>
										<span
											className="block truncate font-mono text-muted-foreground text-xs"
											dir="rtl"
											title={part.path}
										>
											<bdi>{part.path}</bdi>
										</span>
									</span>
									<Badge variant="outline">LOD {part.lod}</Badge>
									<Button
										aria-label={`Show ${part.name} in the file browser`}
										render={<Link {...target} />}
										size="icon-sm"
										variant="ghost"
									>
										<ExternalLinkIcon />
									</Button>
								</li>
							);
						})}
					</ul>
				</Section>
			)}
		</div>
	);
}
