/**
 * Minimal GLB preview (three.js via react-three-fiber). Loaded lazily so three
 * never lands in the shell bundle. The full studio replaces this later.
 */
import { Bounds, Center, OrbitControls, useGLTF } from "@react-three/drei";
import { Canvas } from "@react-three/fiber";
import { BoxIcon, GridIcon, RotateCcwIcon } from "lucide-react";
import * as React from "react";
import type { Object3D } from "three";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Toggle } from "@/components/ui/toggle";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";

interface ModelStats {
	meshes: number;
	triangles: number;
	materials: number;
}

function collectStats(root: Object3D): ModelStats {
	let meshes = 0;
	let triangles = 0;
	const materials = new Set<string>();
	root.traverse((node) => {
		const mesh = node as Object3D & {
			isMesh?: boolean;
			geometry?: {
				index: { count: number } | null;
				attributes: { position?: { count: number } };
			};
			material?: { uuid: string } | { uuid: string }[];
		};
		if (!mesh.isMesh || !mesh.geometry) return;
		meshes++;
		const g = mesh.geometry;
		triangles += Math.floor(
			(g.index ? g.index.count : (g.attributes.position?.count ?? 0)) / 3,
		);
		const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
		for (const m of mats) if (m) materials.add(m.uuid);
	});
	return { meshes, triangles, materials: materials.size };
}

function Model({
	url,
	onStats,
}: {
	url: string;
	onStats: (stats: ModelStats) => void;
}) {
	const { scene } = useGLTF(url);
	React.useEffect(() => {
		onStats(collectStats(scene));
	}, [scene, onStats]);
	return <primitive object={scene} />;
}

class ModelErrorBoundary extends React.Component<
	{ children: React.ReactNode; onError: (error: Error) => void },
	{ failed: boolean }
> {
	state = { failed: false };
	static getDerivedStateFromError() {
		return { failed: true };
	}
	componentDidCatch(error: Error) {
		this.props.onError(error);
	}
	render() {
		return this.state.failed ? null : this.props.children;
	}
}

export default function ModelViewer({
	url,
	className,
}: {
	url: string;
	className?: string;
}): React.ReactElement {
	const [stats, setStats] = React.useState<ModelStats | null>(null);
	const [error, setError] = React.useState<Error | null>(null);
	const [grid, setGrid] = React.useState(true);
	const [resetKey, setResetKey] = React.useState(0);

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset per model
	React.useEffect(() => {
		setStats(null);
		setError(null);
	}, [url]);

	return (
		<div className={cn("flex min-h-0 flex-1 flex-col", className)}>
			<div className="flex h-9 shrink-0 items-center gap-1 border-b px-2 text-muted-foreground text-xs">
				<span className="truncate px-1 tabular-nums">
					{stats
						? `${formatNumber(stats.meshes)} meshes · ${formatNumber(stats.triangles)} tris · ${stats.materials} materials`
						: error
							? "Failed to load"
							: "Loading model…"}
				</span>
				<Toggle
					aria-label="Show grid"
					className="ms-auto"
					onPressedChange={setGrid}
					pressed={grid}
					size="sm"
				>
					<GridIcon />
				</Toggle>
				<Button
					aria-label="Reset camera"
					onClick={() => setResetKey((k) => k + 1)}
					size="icon-sm"
					variant="ghost"
				>
					<RotateCcwIcon />
				</Button>
			</div>
			<div className="relative min-h-0 flex-1 bg-gradient-to-b from-muted/40 to-background">
				{error ? (
					<div className="absolute inset-0 flex flex-col items-center justify-center gap-2 p-6 text-center text-muted-foreground text-sm">
						<BoxIcon className="size-5" />
						<span>Could not load this model.</span>
						<code className="max-w-md break-words text-xs">
							{error.message}
						</code>
					</div>
				) : (
					<>
						{!stats && (
							<div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center">
								<Spinner />
							</div>
						)}
						<Canvas
							aria-label="3D model preview. Drag to orbit, scroll to zoom, right-drag to pan."
							camera={{ fov: 40, near: 0.01, far: 2000, position: [2, 1.5, 3] }}
							dpr={[1, 2]}
							gl={{ antialias: true, preserveDrawingBuffer: false }}
							key={resetKey}
							role="img"
						>
							<hemisphereLight args={["#ffffff", "#666677", 1.6]} />
							<directionalLight intensity={2} position={[3, 5, 4]} />
							<directionalLight intensity={0.6} position={[-4, 2, -3]} />
							<ModelErrorBoundary onError={setError}>
								<React.Suspense fallback={null}>
									<Bounds clip fit margin={1.25} observe>
										<Center top>
											<Model onStats={setStats} url={url} />
										</Center>
									</Bounds>
								</React.Suspense>
							</ModelErrorBoundary>
							{grid && (
								<gridHelper
									args={[20, 40, "#888888", "#444444"]}
									material-opacity={0.25}
									material-transparent
								/>
							)}
							<OrbitControls enableDamping makeDefault />
						</Canvas>
					</>
				)}
			</div>
		</div>
	);
}
