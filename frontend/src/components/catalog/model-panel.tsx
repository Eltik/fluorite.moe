import { Link } from "@tanstack/react-router";
import { BoxesIcon, BoxIcon, ChevronRightIcon, FileIcon } from "lucide-react";
import * as React from "react";
import { Panel } from "@/components/catalog/detail-layout";
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

const ModelPreview = React.lazy(
	() => import("@/components/catalog/model-preview"),
);

/** `body_01` → `Body 01`. */
function partLabel(name: string): string {
	return name
		.replace(/[_-]+/g, " ")
		.replace(/\b\w/g, (c) => c.toUpperCase())
		.trim();
}

/**
 * Model tab: an inline 3D preview with one clear "Open in Studio", and the
 * source files tucked into a disclosure for people who want them.
 */
export function ModelPanel({
	kind,
	id,
	name = id,
	model,
}: {
	kind: CatalogKind;
	id: string;
	name?: string;
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
						No 3D model was exported for this entry.
					</EmptyDescription>
				</EmptyHeader>
			</Empty>
		);
	}
	const studio = isStudioKind(kind) ? kind : null;
	const files = model.rigged
		? [{ name: "Rigged model", path: model.rigged, root: "models" as const }]
		: parts.map((p) => ({
				name: partLabel(p.name),
				path: p.path,
				root: "meshes" as const,
			}));
	return (
		<div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_18rem]">
			{studio ? (
				<React.Suspense
					fallback={
						<div className="aspect-[4/3] max-h-[32rem] w-full rounded-xl border bg-muted/40" />
					}
				>
					<ModelPreview
						className="aspect-[4/3] max-h-[32rem] w-full"
						id={id}
						kind={studio}
						model={model}
						name={name}
					/>
				</React.Suspense>
			) : null}
			<div className="flex flex-col gap-3">
				<Panel className="flex flex-col gap-3">
					<div className="flex items-center gap-3">
						<div className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted text-muted-foreground">
							<BoxesIcon className="size-4.5" />
						</div>
						<div className="min-w-0">
							<div className="font-medium text-sm">
								{model.rigged ? "Rigged model" : "3D model"}
							</div>
							<div className="text-muted-foreground text-xs">
								{model.rigged
									? "Skinned, with textures"
									: `${parts.length} part${parts.length === 1 ? "" : "s"}`}
							</div>
						</div>
					</div>
					{studio && (
						<Button
							className="w-full"
							render={<Link {...studioLink(studio, id)} />}
						>
							<BoxesIcon />
							Open in Studio
						</Button>
					)}
					<p className="text-muted-foreground text-xs">
						Drag to orbit, scroll to zoom.
						{studio && " The Studio adds lighting, backdrops and capture."}
					</p>
				</Panel>
				<details className="group rounded-xl border bg-card text-sm">
					<summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
						<ChevronRightIcon className="size-4 transition-transform group-open:rotate-90" />
						Source files
						<span className="ms-auto text-xs tabular-nums">{files.length}</span>
					</summary>
					<ul className="divide-y border-t">
						{files.map((file) => {
							const inner = (
								<>
									<FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
									<span className="min-w-0 flex-1 truncate">{file.name}</span>
								</>
							);
							const row =
								"flex items-center gap-2 px-3 py-1.5 outline-none hover:bg-accent/50 focus-visible:bg-accent/50";
							return (
								<li key={file.path}>
									{file.root === "meshes" ? (
										<Link
											{...fileTarget("meshes", file.path)}
											className={row}
											title={file.path}
										>
											{inner}
										</Link>
									) : (
										<span className={row} title={file.path}>
											{inner}
										</span>
									)}
								</li>
							);
						})}
					</ul>
				</details>
			</div>
		</div>
	);
}
