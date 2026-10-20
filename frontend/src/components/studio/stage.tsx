/**
 * The Studio's react-three-fiber stage: renderer setup, light rig,
 * environment, ground, camera controls and the mounted model. Rendering is
 * on demand; continuous frames only while animating, auto-rotating or
 * damping the camera.
 */
import {
	CameraControls,
	Environment,
	Grid,
	Lightformer,
} from "@react-three/drei";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as React from "react";
import {
	Box3,
	CanvasTexture,
	type Group,
	MathUtils,
	NeutralToneMapping,
	type PerspectiveCamera,
	SkeletonHelper,
	setConsoleFunction,
	Vector2,
	Vector3,
	type WebGLRenderer,
} from "three";
import type { AnimationController } from "@/components/studio/animation";
import { ContactShadows } from "@/components/studio/contact-shadows";
import type { LoadedModel } from "@/components/studio/load-model";
import type {
	ShadingId,
	StudioPrefs,
	ZUpMode,
} from "@/components/studio/prefs";
import {
	type DirLight,
	LIGHTING,
	type LightingPreset,
	type ResolvedBackground,
} from "@/components/studio/presets";
import { applyShading, shadingUniforms } from "@/components/studio/shading";
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
	zUpMode: ZUpMode;
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
	/** Bumps on every re-frame (keys the baked contact shadow). */
	version: number;
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
	toon,
	radius,
}: {
	preset: LightingPreset;
	toon: boolean;
	radius: number;
}) {
	const d = Math.max(radius, 0.5) * 4;
	const at = (p: [number, number, number]) =>
		new Vector3(...p).normalize().multiplyScalar(d).toArray();
	const lights: DirLight[] = toon
		? [preset.toon.key]
		: [preset.key, preset.fill, preset.rim];
	const ambient = toon ? preset.toon.ambient : preset.ambient;
	const hemi = toon ? preset.toon.hemisphere : preset.hemisphere;
	return (
		<>
			<ambientLight intensity={ambient} />
			<hemisphereLight args={[hemi.sky, hemi.ground, hemi.intensity]} />
			{lights.map((l, i) =>
				l.intensity > 0 ? (
					<directionalLight
						color={l.color}
						intensity={l.intensity}
						// biome-ignore lint/suspicious/noArrayIndexKey: fixed rig slots
						key={i}
						position={at(l.position)}
					/>
				) : null,
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

/** Keeps the toon rim and outline uniforms in sync with the view. */
function ShadingSync({ preset }: { preset: LightingPreset }) {
	const gl = useThree((s) => s.gl);
	const invalidate = useThree((s) => s.invalidate);
	const size = React.useMemo(() => new Vector2(), []);
	React.useEffect(() => {
		shadingUniforms.rimColor.value.set(preset.toon.rim.color);
		shadingUniforms.rimStrength.value = preset.toon.rim.strength;
		invalidate();
	}, [preset, invalidate]);
	useFrame(() => {
		gl.getDrawingBufferSize(size);
		shadingUniforms.resolution.value.copy(size);
		shadingUniforms.outlinePx.value = 1.25 * gl.getPixelRatio();
	});
	return null;
}

// ------------------------------------------------------------- floor

let floorTexture: CanvasTexture | null = null;

/** Radial alpha falloff with a faint ring near the rim. */
function getFloorTexture(): CanvasTexture {
	if (floorTexture) return floorTexture;
	const canvas = document.createElement("canvas");
	canvas.width = canvas.height = 256;
	const ctx = canvas.getContext("2d");
	if (ctx) {
		const g = ctx.createRadialGradient(128, 128, 0, 128, 128, 128);
		g.addColorStop(0, "#ffffff");
		g.addColorStop(0.45, "#e6e6e6");
		g.addColorStop(0.84, "#2a2a2a");
		g.addColorStop(0.9, "#3a3a3a");
		g.addColorStop(0.915, "#5c5c5c");
		g.addColorStop(0.93, "#262626");
		g.addColorStop(1, "#000000");
		ctx.fillStyle = g;
		ctx.fillRect(0, 0, 256, 256);
	}
	floorTexture = new CanvasTexture(canvas);
	return floorTexture;
}

function FloorDisc({
	radius,
	color,
	opacity,
}: {
	radius: number;
	color: string;
	opacity: number;
}) {
	const texture = React.useMemo(getFloorTexture, []);
	return (
		<mesh
			position={[0, -0.003 * radius, 0]}
			raycast={() => {}}
			renderOrder={-1}
			rotation={[-Math.PI / 2, 0, 0]}
		>
			<circleGeometry args={[radius, 96]} />
			<meshBasicMaterial
				alphaMap={texture}
				color={color}
				depthWrite={false}
				opacity={opacity}
				toneMapped={false}
				transparent
			/>
		</mesh>
	);
}

// ------------------------------------------------------------- model

let framingVersion = 0;

function measure(object: Group): Framing {
	object.updateMatrixWorld(true);
	const box = new Box3().setFromObject(object, true);
	if (box.isEmpty())
		box.set(new Vector3(-0.5, 0, -0.5), new Vector3(0.5, 1, 0.5));
	const size = box.getSize(new Vector3());
	const center = box.getCenter(new Vector3());
	return {
		center,
		size,
		radius: Math.max(size.length() / 2, 1e-3),
		version: ++framingVersion,
	};
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
	const distance = Math.max(fitH, fitW) * 1.15 + depth / 2;
	// Depth range sized to the model: close-ups stay crisp, no z-fighting.
	camera.near = Math.max(radius * 0.01, 0.001);
	camera.far = radius * 40;
	camera.updateProjectionMatrix();
	controls.minDistance = radius * 0.08;
	controls.maxDistance = radius * 14;
	shadingUniforms.outlineRef.value = distance;
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
	zUpMode: ZUpMode;
	hiddenParts: ReadonlySet<string>;
	wireframe: boolean;
	shading: ShadingId;
	outlines: boolean;
	onFramed: (framing: Framing | null) => void;
	onShown: (model: LoadedModel | null) => void;
	controls: React.RefObject<CameraControls | null>;
}

const reducedMotion = () =>
	typeof window !== "undefined" &&
	window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Fades the canvas in so a freshly framed model never pops. */
function fadeIn(canvas: HTMLCanvasElement): void {
	if (reducedMotion()) return;
	canvas.style.transition = "none";
	canvas.style.opacity = "0";
	requestAnimationFrame(() => {
		requestAnimationFrame(() => {
			canvas.style.transition = "opacity 280ms cubic-bezier(0.2, 0, 0, 1)";
			canvas.style.opacity = "1";
		});
	});
}

function ModelMount({
	model,
	zUpMode,
	hiddenParts,
	wireframe,
	shading,
	outlines,
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
	const framed = React.useRef<LoadedModel | null>(null);
	const look = React.useRef({ shading, outlines, wireframe });
	look.current = { shading, outlines, wireframe };

	// Pre-compile shaders off-screen (with the final materials), then swap
	// the new model in.
	React.useEffect(() => {
		if (model === current) return;
		if (!model) {
			setCurrent(null);
			return;
		}
		let cancelled = false;
		const l = look.current;
		applyShading(model.content, l.shading, l.outlines, l.wireframe);
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

	// Orientation, ground placement and framing, all before the first paint.
	React.useLayoutEffect(() => {
		const o = outer.current;
		const t = turn.current;
		if (!o || !t) return;
		if (!current) {
			framed.current = null;
			onFramed(null);
			onShown(null);
			return;
		}
		const zUp = zUpMode === "auto" ? current.detectedZUp : zUpMode === "on";
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
		const swapped = framed.current !== current;
		// A new model snaps into place behind a fade; re-orienting the same
		// model glides.
		if (c) frameCamera(c, camera, framing, !swapped);
		if (swapped) fadeIn(gl.domElement);
		framed.current = current;
		invalidate();
		onShown(current);
	}, [current, zUpMode, camera, controls, gl, invalidate, onFramed, onShown]);

	React.useEffect(() => {
		if (!current) return;
		for (const part of current.parts) {
			part.object.visible = !hiddenParts.has(part.id);
		}
		invalidate();
	}, [current, hiddenParts, invalidate]);

	React.useEffect(() => {
		if (!current) return;
		applyShading(current.content, shading, outlines, wireframe);
		invalidate();
	}, [current, shading, outlines, wireframe, invalidate]);

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
	zUpMode,
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
	const footprint = framing
		? Math.max(Math.hypot(framing.size.x, framing.size.z) * 0.5, height * 0.24)
		: 1;
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
			<LightRig
				preset={preset}
				radius={radius}
				toon={prefs.shading === "toon"}
			/>
			<StudioEnvironment key={prefs.lighting} preset={preset} />
			<ShadingSync preset={preset} />
			<ModelMount
				controls={controls}
				hiddenParts={hiddenParts}
				model={model}
				onFramed={setFraming}
				onShown={handleShown}
				outlines={prefs.outlines}
				shading={prefs.shading}
				wireframe={prefs.wireframe}
				zUpMode={zUpMode}
			/>
			{prefs.floor && shown && framing && (
				<FloorDisc
					color={background.floor.color}
					opacity={background.floor.opacity}
					radius={footprint}
				/>
			)}
			{shown?.rigged && prefs.skeleton && <Skeleton model={shown} />}
			{prefs.groundShadow && shown && framing && (
				<ContactShadows
					bakeKey={`${framing.version}:${visibilityKey}:${prefs.wireframe}:${prefs.shading}:${prefs.outlines}`}
					blur={2.2}
					color={background.dark ? "#000000" : "#2a2440"}
					far={Math.max(height * 0.4, 0.2)}
					frames={animating ? Number.POSITIVE_INFINITY : 1}
					opacity={background.dark ? 0.8 : 0.55}
					position={[0, 0, 0]}
					resolution={1024}
					scale={footprint * 2.6}
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
