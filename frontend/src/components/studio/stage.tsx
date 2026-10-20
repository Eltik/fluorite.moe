/**
 * The Studio's react-three-fiber stage: renderer setup, light rig,
 * environment, ground, camera controls and the mounted model. Rendering is
 * on demand; continuous frames only while animating, auto-rotating or
 * damping the camera.
 */
import {
	CameraControls,
	ContactShadows,
	Environment,
	Grid,
	Lightformer,
} from "@react-three/drei";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as React from "react";
import {
	Box3,
	type Group,
	MathUtils,
	type Mesh,
	NeutralToneMapping,
	type PerspectiveCamera,
	SkeletonHelper,
	setConsoleFunction,
	Vector3,
	type WebGLRenderer,
} from "three";
import type { AnimationController } from "@/components/studio/animation";
import type { LoadedModel } from "@/components/studio/load-model";
import type { StudioPrefs } from "@/components/studio/prefs";
import {
	LIGHTING,
	type LightingPreset,
	type ResolvedBackground,
} from "@/components/studio/presets";
import { publishStats } from "@/components/studio/stats-store";

// r3f still constructs THREE.Clock, whose deprecation warning fires on every
// canvas mount. Drop just that message; everything else passes through.
setConsoleFunction((type: string, message: string, ...params: unknown[]) => {
	if (type === "warn" && message.startsWith("THREE.Clock:")) return;
	const log =
		type === "error"
			? console.error
			: type === "warn"
				? console.warn
				: console.log;
	log(message, ...params);
});

export interface StageHandle {
	resetView: () => void;
	screenshot: (filename: string) => Promise<void>;
}

export interface StageProps {
	model: LoadedModel | null;
	zUp: boolean;
	hiddenParts: ReadonlySet<string>;
	prefs: StudioPrefs;
	background: ResolvedBackground;
	locked: boolean;
	animation: AnimationController | null;
	handleRef: React.RefObject<StageHandle | null>;
	/** Called once a model is compiled, attached and framed. */
	onShown: (model: LoadedModel | null) => void;
	onContextLost: (lost: boolean) => void;
	label: string;
}

interface Framing {
	center: Vector3;
	size: Vector3;
	radius: number;
}

const DEFAULT_AZIMUTH = MathUtils.degToRad(28);
const DEFAULT_POLAR = MathUtils.degToRad(80);

// ------------------------------------------------------------- renderer

function RendererSettings({ exposure }: { exposure: number }) {
	const gl = useThree((s) => s.gl);
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		gl.toneMapping = NeutralToneMapping;
		gl.toneMappingExposure = exposure;
		invalidate();
	}, [gl, exposure, invalidate]);
	return null;
}

/**
 * Adaptive pixel ratio: capped by the user setting, lowered while the camera
 * moves (drei `regress`), and stepped down when sustained frame rate drops.
 */
function AdaptivePixelRatio({ cap }: { cap: number }) {
	const setDpr = useThree((s) => s.setDpr);
	const current = useThree((s) => s.performance.current);
	const [factor, setFactor] = React.useState(1);
	const samples = React.useRef<number[]>([]);

	useFrame((_, delta) => {
		// Only consecutive frames count; demand-mode gaps are not slowness.
		if (delta > 0.1) {
			samples.current.length = 0;
			return;
		}
		const s = samples.current;
		s.push(delta);
		if (s.length < 90) return;
		const fps = s.length / s.reduce((a, b) => a + b, 0);
		s.length = 0;
		if (fps < 40) setFactor((f) => Math.max(0.6, f - 0.15));
		else if (fps > 57) setFactor((f) => Math.min(1, f + 0.1));
	});

	React.useEffect(() => {
		const device = window.devicePixelRatio || 1;
		const base = Math.min(device, cap) * factor;
		const floor = Math.min(1, base);
		setDpr(Math.max(floor, base * current));
	}, [cap, factor, current, setDpr]);
	return null;
}

function StatsProbe() {
	const gl = useThree((s) => s.gl);
	const frames = React.useRef(0);
	useFrame(() => {
		frames.current++;
	});
	React.useEffect(() => {
		let last = performance.now();
		const id = window.setInterval(() => {
			const now = performance.now();
			const fps = (frames.current * 1000) / (now - last);
			frames.current = 0;
			last = now;
			publishStats({
				fps,
				calls: gl.info.render.calls,
				triangles: gl.info.render.triangles,
				geometries: gl.info.memory.geometries,
				textures: gl.info.memory.textures,
				dpr: gl.getPixelRatio(),
				width: gl.domElement.width,
				height: gl.domElement.height,
			});
		}, 500);
		return () => window.clearInterval(id);
	}, [gl]);
	return null;
}

// ------------------------------------------------------------- lighting

function LightRig({
	preset,
	radius,
}: {
	preset: LightingPreset;
	radius: number;
}) {
	const d = Math.max(radius, 0.5) * 4;
	const at = (p: [number, number, number]) =>
		new Vector3(...p).normalize().multiplyScalar(d).toArray();
	return (
		<>
			<ambientLight intensity={preset.ambient} />
			<hemisphereLight
				args={[
					preset.hemisphere.sky,
					preset.hemisphere.ground,
					preset.hemisphere.intensity,
				]}
			/>
			{preset.key.intensity > 0 && (
				<directionalLight
					color={preset.key.color}
					intensity={preset.key.intensity}
					position={at(preset.key.position)}
				/>
			)}
			{preset.fill.intensity > 0 && (
				<directionalLight
					color={preset.fill.color}
					intensity={preset.fill.intensity}
					position={at(preset.fill.position)}
				/>
			)}
			{preset.rim.intensity > 0 && (
				<directionalLight
					color={preset.rim.color}
					intensity={preset.rim.intensity}
					position={at(preset.rim.position)}
				/>
			)}
		</>
	);
}

/** Procedural studio environment: lightformers only, no HDR downloads. */
function StudioEnvironment({ preset }: { preset: LightingPreset }) {
	return (
		<Environment
			environmentIntensity={preset.environment}
			frames={1}
			resolution={256}
		>
			{preset.formers.map((f, i) => (
				<Lightformer
					color={f.color ?? "#ffffff"}
					form={f.form}
					intensity={f.intensity}
					// biome-ignore lint/suspicious/noArrayIndexKey: static preset list
					key={i}
					position={f.position}
					rotation={f.rotation}
					scale={f.scale}
					target={f.target === false ? undefined : [0, 0, 0]}
				/>
			))}
		</Environment>
	);
}

// ------------------------------------------------------------- model

function measure(object: Group): Framing {
	object.updateMatrixWorld(true);
	const box = new Box3().setFromObject(object, true);
	if (box.isEmpty())
		box.set(new Vector3(-0.5, 0, -0.5), new Vector3(0.5, 1, 0.5));
	const size = box.getSize(new Vector3());
	const center = box.getCenter(new Vector3());
	return { center, size, radius: Math.max(size.length() / 2, 1e-3) };
}

function frameCamera(
	controls: CameraControls,
	camera: PerspectiveCamera,
	framing: Framing,
	transition: boolean,
): void {
	const { center, size, radius } = framing;
	const vFov = MathUtils.degToRad(camera.fov);
	const hFov = 2 * Math.atan(Math.tan(vFov / 2) * camera.aspect);
	const depth = Math.max(size.x, size.z);
	const fitH = size.y / 2 / Math.tan(vFov / 2);
	const fitW = Math.max(size.x, size.z) / 2 / Math.tan(hFov / 2);
	const distance = Math.max(fitH, fitW) * 1.22 + depth / 2;
	camera.near = Math.max(radius / 500, 0.001);
	camera.far = radius * 400;
	camera.updateProjectionMatrix();
	controls.minDistance = radius * 0.08;
	controls.maxDistance = radius * 14;
	const offset = new Vector3().setFromSphericalCoords(
		distance,
		DEFAULT_POLAR,
		DEFAULT_AZIMUTH,
	);
	// Aim slightly below centre so the model clears the bottom toolbar.
	const target = center.clone().setY(center.y - size.y * 0.07);
	const position = target.clone().add(offset);
	void controls.setLookAt(
		position.x,
		position.y,
		position.z,
		target.x,
		target.y,
		target.z,
		transition,
	);
	controls.saveState();
}

interface MountProps {
	model: LoadedModel | null;
	zUp: boolean;
	hiddenParts: ReadonlySet<string>;
	wireframe: boolean;
	onFramed: (framing: Framing | null) => void;
	onShown: (model: LoadedModel | null) => void;
	controls: React.RefObject<CameraControls | null>;
}

function ModelMount({
	model,
	zUp,
	hiddenParts,
	wireframe,
	onFramed,
	onShown,
	controls,
}: MountProps) {
	const gl = useThree((s) => s.gl);
	const camera = useThree((s) => s.camera) as PerspectiveCamera;
	const scene = useThree((s) => s.scene);
	const invalidate = useThree((s) => s.invalidate);
	const outer = React.useRef<Group>(null);
	const turn = React.useRef<Group>(null);
	const [current, setCurrent] = React.useState<LoadedModel | null>(null);
	const firstFrame = React.useRef(true);

	// Pre-compile shaders off-screen, then swap the new model in.
	React.useEffect(() => {
		if (model === current) return;
		if (!model) {
			setCurrent(null);
			return;
		}
		let cancelled = false;
		(gl as WebGLRenderer)
			.compileAsync(model.content, camera, scene)
			.catch(() => {})
			.finally(() => {
				if (!cancelled) setCurrent(model);
			});
		return () => {
			cancelled = true;
		};
	}, [model, current, gl, camera, scene]);

	React.useLayoutEffect(() => {
		const holder = turn.current;
		if (!holder || !current) return;
		holder.add(current.content);
		return () => {
			holder.remove(current.content);
		};
	}, [current]);

	// Orientation, ground placement and framing.
	React.useLayoutEffect(() => {
		const o = outer.current;
		const t = turn.current;
		if (!o || !t) return;
		if (!current) {
			onFramed(null);
			onShown(null);
			return;
		}
		t.rotation.set(zUp ? -Math.PI / 2 : 0, 0, 0);
		o.position.set(0, 0, 0);
		const raw = measure(o);
		o.position.set(
			-raw.center.x,
			-(raw.center.y - raw.size.y / 2),
			-raw.center.z,
		);
		const framing = measure(o);
		onFramed(framing);
		const c = controls.current;
		if (c) frameCamera(c, camera, framing, !firstFrame.current);
		firstFrame.current = false;
		invalidate();
		onShown(current);
	}, [current, zUp, camera, controls, invalidate, onFramed, onShown]);

	React.useEffect(() => {
		if (!current) return;
		for (const part of current.parts) {
			part.object.visible = !hiddenParts.has(part.id);
		}
		invalidate();
	}, [current, hiddenParts, invalidate]);

	React.useEffect(() => {
		if (!current) return;
		current.content.traverse((node) => {
			const mesh = node as Mesh;
			if (!mesh.isMesh) return;
			const mats = Array.isArray(mesh.material)
				? mesh.material
				: [mesh.material];
			for (const m of mats) {
				if (m && "wireframe" in m)
					(m as { wireframe: boolean }).wireframe = wireframe;
			}
		});
		invalidate();
	}, [current, wireframe, invalidate]);

	return (
		<group ref={outer}>
			<group ref={turn} />
		</group>
	);
}

function Skeleton({ model }: { model: LoadedModel }) {
	const helper = React.useMemo(
		() => new SkeletonHelper(model.content),
		[model],
	);
	React.useEffect(() => () => helper.dispose(), [helper]);
	return helper.bones.length ? <primitive object={helper} /> : null;
}

// ------------------------------------------------------------- drivers

function AutoRotate({
	controls,
	enabled,
}: {
	controls: React.RefObject<CameraControls | null>;
	enabled: boolean;
}) {
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		if (enabled) invalidate();
	}, [enabled, invalidate]);
	useFrame((_, delta) => {
		if (!enabled) return;
		const c = controls.current;
		if (c?.enabled && c.currentAction === 0) {
			void c.rotate(Math.min(delta, 0.1) * 0.35, 0, false);
		}
		invalidate();
	});
	return null;
}

function MixerDriver({
	controller,
}: {
	controller: AnimationController | null;
}) {
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		if (!controller) return;
		controller.invalidate = invalidate;
		invalidate();
		return () => {
			controller.invalidate = () => {};
		};
	}, [controller, invalidate]);
	useFrame((_, delta) => {
		if (!controller?.state.playing) return;
		controller.update(Math.min(delta, 0.1));
		invalidate();
	});
	return null;
}

function HandleBridge({
	handleRef,
	controls,
	background,
}: {
	handleRef: React.RefObject<StageHandle | null>;
	controls: React.RefObject<CameraControls | null>;
	background: ResolvedBackground;
}) {
	const gl = useThree((s) => s.gl);
	const scene = useThree((s) => s.scene);
	const camera = useThree((s) => s.camera);
	React.useImperativeHandle(
		handleRef,
		() => ({
			resetView: () => {
				void controls.current?.reset(true);
			},
			screenshot: async (filename: string) => {
				gl.render(scene, camera);
				const src = gl.domElement;
				const out = document.createElement("canvas");
				out.width = src.width;
				out.height = src.height;
				const ctx = out.getContext("2d");
				if (!ctx) throw new Error("Canvas 2D is unavailable");
				background.paint(ctx, out.width, out.height);
				ctx.drawImage(src, 0, 0);
				const blob = await new Promise<Blob | null>((resolve) =>
					out.toBlob(resolve, "image/png"),
				);
				if (!blob) throw new Error("Could not encode the screenshot");
				const url = URL.createObjectURL(blob);
				const a = document.createElement("a");
				a.href = url;
				a.download = filename;
				a.click();
				window.setTimeout(() => URL.revokeObjectURL(url), 1000);
			},
		}),
		[gl, scene, camera, controls, background],
	);
	return null;
}

function ControlsLock({
	controls,
	locked,
}: {
	controls: React.RefObject<CameraControls | null>;
	locked: boolean;
}) {
	React.useEffect(() => {
		if (controls.current) controls.current.enabled = !locked;
	}, [controls, locked]);
	return null;
}

// ------------------------------------------------------------- stage

export function Stage({
	model,
	zUp,
	hiddenParts,
	prefs,
	background,
	locked,
	animation,
	handleRef,
	onShown,
	onContextLost,
	label,
}: StageProps): React.ReactElement {
	const controls = React.useRef<CameraControls | null>(null);
	const [framing, setFraming] = React.useState<Framing | null>(null);
	const [shown, setShown] = React.useState<LoadedModel | null>(null);
	const preset = LIGHTING[prefs.lighting];
	const radius = framing?.radius ?? 1;
	const height = framing?.size.y ?? 1;
	const animating = Boolean(animation);

	const handleShown = React.useCallback(
		(m: LoadedModel | null) => {
			setShown(m);
			onShown(m);
		},
		[onShown],
	);

	const visibilityKey = React.useMemo(
		() => [...hiddenParts].sort().join("|"),
		[hiddenParts],
	);

	return (
		<Canvas
			aria-label={label}
			camera={{ fov: 32, near: 0.01, far: 500, position: [1.6, 1.3, 4.2] }}
			className="touch-none"
			dpr={[1, prefs.dprCap]}
			frameloop="demand"
			gl={{ antialias: true, alpha: true, powerPreference: "high-performance" }}
			onCreated={({ gl }) => {
				gl.toneMapping = NeutralToneMapping;
				gl.setClearColor(0x000000, 0);
				const canvas = gl.domElement;
				canvas.addEventListener("webglcontextlost", () => onContextLost(true));
				canvas.addEventListener("webglcontextrestored", () =>
					onContextLost(false),
				);
			}}
			performance={{ min: 0.75, debounce: 250 }}
			role="img"
		>
			<RendererSettings exposure={prefs.exposure} />
			<AdaptivePixelRatio cap={prefs.dprCap} />
			{prefs.stats && <StatsProbe />}
			<LightRig preset={preset} radius={radius} />
			<StudioEnvironment key={prefs.lighting} preset={preset} />
			<ModelMount
				controls={controls}
				hiddenParts={hiddenParts}
				model={model}
				onFramed={setFraming}
				onShown={handleShown}
				wireframe={prefs.wireframe}
				zUp={zUp}
			/>
			{shown?.rigged && prefs.skeleton && <Skeleton model={shown} />}
			{prefs.groundShadow && shown && framing && (
				<ContactShadows
					blur={2.6}
					color={background.dark ? "#000000" : "#1c1a2b"}
					far={Math.max(height * 0.5, 0.25)}
					frames={animating ? Number.POSITIVE_INFINITY : 1}
					key={`${shown.key}:${zUp}:${visibilityKey}:${prefs.wireframe}`}
					opacity={background.dark ? 0.7 : 0.45}
					position={[0, 0, 0]}
					resolution={1024}
					scale={radius * 4}
				/>
			)}
			{prefs.grid && (
				<Grid
					cellColor={background.dark ? "#4b4760" : "#b9b6c8"}
					cellSize={radius * 0.1}
					cellThickness={0.6}
					fadeDistance={radius * 10}
					fadeStrength={1.5}
					infiniteGrid
					position={[0, -0.002 * radius, 0]}
					sectionColor={background.dark ? "#7c6fb0" : "#8b80b8"}
					sectionSize={radius * 0.5}
					sectionThickness={1}
				/>
			)}
			<CameraControls
				dollySpeed={0.6}
				makeDefault
				ref={controls}
				regress
				smoothTime={0.2}
			/>
			<ControlsLock controls={controls} locked={locked} />
			<AutoRotate controls={controls} enabled={prefs.autoRotate && !locked} />
			<MixerDriver controller={animation} />
			<HandleBridge
				background={background}
				controls={controls}
				handleRef={handleRef}
			/>
		</Canvas>
	);
}
