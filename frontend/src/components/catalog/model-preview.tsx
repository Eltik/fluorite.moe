/**
 * Inline 3D preview for catalog Model tabs. Uses the Studio's loader (same
 * textures and part handling) on a small orbitable stage; the full Studio is
 * one click away. Loaded lazily so three never lands in the catalog chunk.
 */
import { Bounds, OrbitControls } from "@react-three/drei";
import { Canvas } from "@react-three/fiber";
import { BoxIcon } from "lucide-react";
import * as React from "react";
import { type LoadedModel, loadModel } from "@/components/studio/load-model";
import { Spinner } from "@/components/ui/spinner";
import type { CatalogModel } from "@/lib/api";
import type { StudioKind } from "@/lib/studio";
import { cn } from "@/lib/utils";

export default function ModelPreview({
	kind,
	id,
	name,
	model,
	className,
}: {
	kind: StudioKind;
	id: string;
	name: string;
	model: CatalogModel;
	className?: string;
}): React.ReactElement {
	const [loaded, setLoaded] = React.useState<LoadedModel | null>(null);
	const [progress, setProgress] = React.useState(0);
	const [error, setError] = React.useState<string | null>(null);

	React.useEffect(() => {
		const controller = new AbortController();
		let current: LoadedModel | null = null;
		setLoaded(null);
		setError(null);
		setProgress(0);
		loadModel(
			{
				entry: { kind, id, name, tags: [] },
				rigged: model.rigged,
				parts: model.parts.map((p) => p.path),
				source: "api",
			},
			{
				signal: controller.signal,
				onProgress: (done, total) =>
					setProgress(total > 0 ? Math.min(1, done / total) : 0),
			},
		).then(
			(result) => {
				if (controller.signal.aborted) {
					result.dispose();
					return;
				}
				current = result;
				setLoaded(result);
			},
			(e: unknown) => {
				if (controller.signal.aborted) return;
				setError(e instanceof Error ? e.message : String(e));
			},
		);
		return () => {
			controller.abort();
			current?.dispose();
		};
	}, [kind, id, name, model]);

	return (
		<div
			className={cn(
				"relative overflow-hidden rounded-xl border bg-[radial-gradient(90%_80%_at_50%_100%,var(--color-muted),var(--color-card))]",
				className,
			)}
		>
			{error ? (
				<div className="absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center text-muted-foreground text-sm">
					<BoxIcon className="size-5" />
					Could not load a preview of this model.
				</div>
			) : !loaded ? (
				<div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-muted-foreground text-xs">
					<Spinner />
					<span className="tabular-nums">
						Loading model
						{progress > 0 ? ` · ${Math.round(progress * 100)}%` : "…"}
					</span>
				</div>
			) : (
				<Canvas
					aria-label={`3D preview of ${name}. Drag to orbit, scroll to zoom.`}
					camera={{ fov: 30, near: 0.01, far: 500, position: [0, 1, 4] }}
					dpr={[1, 1.5]}
					frameloop="demand"
					role="img"
				>
					<hemisphereLight args={["#ffffff", "#5a5a66", 2.2]} />
					<directionalLight intensity={2.4} position={[2, 4, 5]} />
					<directionalLight intensity={0.8} position={[-4, 2, -3]} />
					<Bounds clip fit margin={1.1}>
						<primitive
							object={loaded.content}
							rotation={[loaded.detectedZUp ? -Math.PI / 2 : 0, 0, 0]}
						/>
					</Bounds>
					<OrbitControls enableDamping={false} enablePan={false} makeDefault />
				</Canvas>
			)}
		</div>
	);
}
