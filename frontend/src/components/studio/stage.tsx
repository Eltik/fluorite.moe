/**
 * The Studio's react-three-fiber stage: renderer setup, light rig,
 * environment, ground, camera controls and the mounted model. Rendering is
 * on demand; continuous frames only while animating, auto-rotating or
 * damping the camera.
 */
import {
	CameraControls,
	CameraControlsImpl,
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
	type Mesh,
	NeutralToneMapping,
	NoToneMapping,
	type PerspectiveCamera,
	type Quaternion,
	SkeletonHelper,
	type SkinnedMesh,
	setConsoleFunction,
	Vector2,
	Vector3,
	type WebGLRenderer,
} from "three";
import type { AnimationController } from "@/components/studio/animation";
import { ContactShadows } from "@/components/studio/contact-shadows";
import type { LoadedModel } from "@/components/studio/load-model";
import { StageBackdrop } from "@/components/studio/npr/backdrop";
import { Bloom } from "@/components/studio/npr/bloom";
import { ContactBlobs } from "@/components/studio/npr/contact-blobs";
import {
	AdditionalLights,
	clearAdditionalLights,
} from "@/components/studio/npr/lights";
import {
	applyVolume,
	FRONT_VIEW_FORWARD,
	keyLightDirection,
	rigUniforms,
	shadowUniforms,
} from "@/components/studio/npr/rig";
import { SelfShadow } from "@/components/studio/npr/self-shadow";
import { useSceneObjects } from "@/components/studio/npr/use-scene-objects";
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
import { PropsRig } from "@/components/studio/props-rig";
import {
	applyShading,
	shadingUniforms,
	updateFaceFrames,
} from "@/components/studio/shading";
import { publishStats } from "@/components/studio/stats-store";
import {
	cameraDirection,
	FRONT_DIRECTION,
	fitBox,
	type Insets,
	STATIC_DIRECTION,
	type Vec3,
} from "@/lib/framing";

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
	/** Back to the default view, re-fitted to the current viewport. */
	resetView: () => void;
	/** Pans by a fraction of the view (1 = a full frame). */
	pan: (dx: number, dy: number) => void;
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
	/** Non-fatal problems while loading the weapon or props. */
	onPropWarnings: (warnings: string[]) => void;
	onContextLost: (lost: boolean) => void;
	label: string;
}

interface Framing {
	center: Vector3;
	size: Vector3;
	min: Vector3;
	max: Vector3;
	radius: number;
	/** Bumps on every re-frame (keys the baked contact shadow). */
	version: number;
}

/** The game's character menu camera. */
const CAMERA_FOV = 20;
/** Room around the fitted figure. */
const FIT_MARGIN = 1.06;
/** Interface chrome in CSS pixels, converted to viewport fractions. */
const CHROME = { top: 76, bottomBare: 72, bottomTransport: 128, side: 24 };

// ------------------------------------------------------------- renderer

/**
 * Toon mode tone maps inside the character materials and leaves the renderer
 * at NoToneMapping; the standard look uses Neutral with the exposure slider.
 */
function RendererSettings({
	exposure,
	toon,
}: {
	exposure: number;
	toon: boolean;
}) {
	const gl = useThree((s) => s.gl);
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		gl.toneMapping = toon ? NoToneMapping : NeutralToneMapping;
		gl.toneMappingExposure = exposure;
		invalidate();
	}, [gl, exposure, toon, invalidate]);
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

/**
 * Drives the NPR rig every frame: the key light follows the camera (yaw plus
 * bias, pitch clamped) using the shown model's volume, and SDF-lit faces get
 * their current head axes.
 */
function NprSync({
	model,
	follow,
	extraLights,
	selfShadow,
	groundShadow,
	stageFactor,
}: {
	model: LoadedModel | null;
	follow: boolean;
	extraLights: boolean;
	selfShadow: boolean;
	groundShadow: boolean;
	/** Dims the additional lights (0.5 on the dark stage). */
	stageFactor: number;
}) {
	const gl = useThree((s) => s.gl);
	const scene = useThree((s) => s.scene);
	const camera = useThree((s) => s.camera);
	const invalidate = useThree((s) => s.invalidate);
	const forward = React.useMemo(() => new Vector3(), []);
	const lights = React.useMemo(
		() =>
			model && extraLights && model.lights.length
				? new AdditionalLights(model.content, model.lights)
				: null,
		[model, extraLights],
	);
	const shadow = React.useMemo(
		() => (model?.rigged && selfShadow ? new SelfShadow(model.content) : null),
		[model, selfShadow],
	);
	React.useEffect(() => () => shadow?.dispose(), [shadow]);
	React.useEffect(() => {
		if (model) applyVolume(model.volume);
		if (!lights) clearAdditionalLights();
		if (!shadow) shadowUniforms.uShadowOn.value = 0;
		invalidate();
	}, [model, lights, shadow, invalidate]);
	const ground = React.useMemo(
		() => (shadow && groundShadow ? [shadow.ground] : []),
		[shadow, groundShadow],
	);
	useSceneObjects(ground);
	useFrame(() => {
		if (!model) return;
		const dir = follow ? camera.getWorldDirection(forward) : FRONT_VIEW_FORWARD;
		keyLightDirection(dir, model.volume, rigUniforms.uLightDir.value);
		updateFaceFrames(model.content);
		lights?.update(stageFactor);
		shadow?.render(gl, scene, 0);
	});
	return null;
}

/** Keeps the fallback toon rim and outline uniforms in sync with the view. */
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

/**
 * Bounds of what actually draws: hidden parts (LOD levels, shadow proxies,
 * VFX meshes that sit far from the body) must not pull the framing, and
 * skinned meshes are measured through their bones (their geometry sits in
 * bind space, which can be far from where the skeleton puts it).
 */
function measure(object: Group): Framing {
	object.updateMatrixWorld(true);
	const box = new Box3();
	const part = new Box3();
	const visit = (node: Group | Mesh) => {
		if (!node.visible) return;
		const mesh = node as Mesh;
		if (mesh.isMesh && mesh.geometry) {
			const skinned = mesh as SkinnedMesh;
			if (skinned.isSkinnedMesh) {
				skinned.skeleton.update();
				skinned.computeBoundingBox();
				if (skinned.boundingBox) {
					part.copy(skinned.boundingBox).applyMatrix4(mesh.matrixWorld);
					box.union(part);
				}
			} else {
				const g = mesh.geometry;
				if (!g.boundingBox) g.computeBoundingBox();
				if (g.boundingBox) {
					part.copy(g.boundingBox).applyMatrix4(mesh.matrixWorld);
					box.union(part);
				}
			}
		}
		for (const child of node.children) visit(child as Group);
	};
	visit(object);
	if (box.isEmpty())
		box.set(new Vector3(-0.5, 0, -0.5), new Vector3(0.5, 1, 0.5));
	const size = box.getSize(new Vector3());
	const center = box.getCenter(new Vector3());
	return {
		center,
		size,
		min: box.min.clone(),
		max: box.max.clone(),
		radius: Math.max(size.length() / 2, 1e-3),
		version: ++framingVersion,
	};
}

/** Chrome fractions for a viewport, with room for the transport bar. */
function insetsFor(width: number, height: number, transport: boolean): Insets {
	const h = Math.max(height, 1);
	const w = Math.max(width, 1);
	return {
		top: Math.min(CHROME.top / h, 0.3),
		bottom: Math.min(
			(transport ? CHROME.bottomTransport : CHROME.bottomBare) / h,
			0.35,
		),
		left: Math.min(CHROME.side / w, 0.2),
		right: Math.min(CHROME.side / w, 0.2),
	};
}

/**
 * The default view: the game's character-screen camera direction and FOV
 * where the catalog has them (centred, whole figure in view; the game's lens
 * shift for its own UI is dropped), else a front view for characters and a
 * three-quarter view for static models. The fit uses the free part of the
 * viewport, so it holds on phones too.
 */
function frameCamera(
	controls: CameraControls,
	camera: PerspectiveCamera,
	framing: Framing,
	model: LoadedModel,
	orientation: Quaternion,
	viewport: { width: number; height: number },
	transition: boolean,
): void {
	const game = model.charscreen?.camera ?? null;
	const dir = new Vector3(
		...(game
			? cameraDirection(game)
			: model.rigged
				? FRONT_DIRECTION
				: STATIC_DIRECTION),
	).applyQuaternion(orientation);
	const fov = game ? game.fov : CAMERA_FOV;
	const fit = fitBox({
		min: framing.min.toArray() as Vec3,
		max: framing.max.toArray() as Vec3,
		direction: dir.toArray() as Vec3,
		fov,
		aspect: viewport.width / Math.max(viewport.height, 1),
		insets: insetsFor(
			viewport.width,
			viewport.height,
			Boolean(model.clips?.entries.length),
		),
		margin: FIT_MARGIN,
	});
	const { radius } = framing;
	camera.fov = fov;
	// Depth range sized to the model (and the stage dome): close-ups stay
	// crisp, no z-fighting.
	camera.near = Math.max(radius * 0.01, 0.001);
	camera.far = Math.max(radius * 40, 60);
	camera.updateProjectionMatrix();
	controls.minDistance = radius * 0.08;
	controls.maxDistance = radius * 14;
	shadingUniforms.outlineRef.value = fit.distance;
	void controls.setLookAt(
		fit.position[0],
		fit.position[1],
		fit.position[2],
		fit.target[0],
		fit.target[1],
		fit.target[2],
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
	/** Receives a function that re-frames the shown model (Reset). */
	reframeRef: React.RefObject<((transition: boolean) => void) | null>;
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
	reframeRef,
}: MountProps) {
	const gl = useThree((s) => s.gl);
	const camera = useThree((s) => s.camera) as PerspectiveCamera;
	const scene = useThree((s) => s.scene);
	const invalidate = useThree((s) => s.invalidate);
	const size = useThree((s) => s.size);
	const sizeRef = React.useRef(size);
	sizeRef.current = size;
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
	// `hiddenParts` only matters for the measurement when the model lands;
	// toggling parts later must not re-frame.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see above
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
		// Default-hidden helpers are applied below in an effect; measure
		// without them from the start.
		for (const part of current.parts) {
			part.object.visible = !hiddenParts.has(part.id);
		}
		const raw = measure(o);
		o.position.set(
			-raw.center.x,
			-(raw.center.y - raw.size.y / 2),
			-raw.center.z,
		);
		const framing = measure(o);
		onFramed(framing);
		const swapped = framed.current !== current;
		const reframe = (transition: boolean) => {
			const c = controls.current;
			if (c) {
				frameCamera(
					c,
					camera,
					framing,
					current,
					t.quaternion,
					sizeRef.current,
					transition,
				);
			}
		};
		reframeRef.current = reframe;
		// A new model snaps into place behind a fade; re-orienting the same
		// model glides.
		reframe(!swapped);
		if (swapped) fadeIn(gl.domElement);
		framed.current = current;
		invalidate();
		onShown(current);
		return () => {
			if (reframeRef.current === reframe) reframeRef.current = null;
		};
	}, [
		current,
		zUpMode,
		camera,
		controls,
		gl,
		invalidate,
		onFramed,
		onShown,
		reframeRef,
	]);

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
	reframeRef,
}: {
	handleRef: React.RefObject<StageHandle | null>;
	controls: React.RefObject<CameraControls | null>;
	background: ResolvedBackground;
	reframeRef: React.RefObject<((transition: boolean) => void) | null>;
}) {
	const gl = useThree((s) => s.gl);
	const scene = useThree((s) => s.scene);
	const camera = useThree((s) => s.camera);
	const invalidate = useThree((s) => s.invalidate);
	React.useImperativeHandle(
		handleRef,
		() => ({
			resetView: () => {
				const c = controls.current;
				if (!c) return;
				if (reframeRef.current) reframeRef.current(true);
				else void c.reset(true);
				invalidate();
			},
			pan: (dx, dy) => {
				const c = controls.current;
				if (!c?.enabled) return;
				// A tenth of the visible height per key press.
				const cam = c.camera as PerspectiveCamera;
				const visible = 2 * c.distance * Math.tan((cam.fov * Math.PI) / 360);
				void c.truck(dx * visible * 0.1, dy * visible * 0.1, true);
				invalidate();
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
		[gl, scene, camera, controls, background, reframeRef, invalidate],
	);
	return null;
}

/**
 * Shift- or Ctrl-drag with the left button pans, for trackpads without a
 * right button. The modifier is read on pointerdown (before camera-controls
 * sees it) and the button mapping restored on release. Right-drag and
 * two-finger drag pan through camera-controls' defaults.
 */
function PanModifiers({
	controls,
}: {
	controls: React.RefObject<CameraControls | null>;
}) {
	const gl = useThree((s) => s.gl);
	React.useEffect(() => {
		const el = gl.domElement;
		const onDown = (e: PointerEvent) => {
			const c = controls.current;
			if (!c?.enabled || e.button !== 0 || !(e.shiftKey || e.ctrlKey)) return;
			c.mouseButtons.left = CameraControlsImpl.ACTION.TRUCK;
			const restore = () => {
				c.mouseButtons.left = CameraControlsImpl.ACTION.ROTATE;
				window.removeEventListener("pointerup", restore, true);
				window.removeEventListener("pointercancel", restore, true);
			};
			window.addEventListener("pointerup", restore, true);
			window.addEventListener("pointercancel", restore, true);
		};
		// Ctrl-click is a context menu on macOS; keep the drag alive.
		const onMenu = (e: Event) => e.preventDefault();
		el.addEventListener("pointerdown", onDown, true);
		el.addEventListener("contextmenu", onMenu);
		return () => {
			el.removeEventListener("pointerdown", onDown, true);
			el.removeEventListener("contextmenu", onMenu);
		};
	}, [gl, controls]);
	return null;
}

/**
 * The model's default weapon and deco props: loaded once the model is shown,
 * shaded like it, shown or hidden per clip time.
 */
function CharProps({
	model,
	controller,
	shading,
	outlines,
	wireframe,
	showWeapon,
	showProps,
	onWarnings,
}: {
	model: LoadedModel | null;
	controller: AnimationController | null;
	shading: ShadingId;
	outlines: boolean;
	wireframe: boolean;
	showWeapon: boolean;
	showProps: boolean;
	onWarnings: (warnings: string[]) => void;
}) {
	const gl = useThree((s) => s.gl);
	const camera = useThree((s) => s.camera);
	const scene = useThree((s) => s.scene);
	const invalidate = useThree((s) => s.invalidate);
	const rig = React.useRef<PropsRig | null>(null);
	const look = React.useRef({ shading, outlines, wireframe });
	look.current = { shading, outlines, wireframe };
	const enabled = React.useRef({ showWeapon, showProps });
	enabled.current = { showWeapon, showProps };

	React.useEffect(() => {
		rig.current = null;
		const cs = model?.charscreen;
		if (!model || !cs || (!cs.weapon?.glb && !cs.deco?.items.length)) return;
		const r = new PropsRig(model);
		const aborter = new AbortController();
		void r.load(aborter.signal, look.current).then(
			() => {
				if (aborter.signal.aborted) return;
				r.setEnabled(enabled.current.showWeapon, enabled.current.showProps);
				rig.current = r;
				if (r.warnings.length) onWarnings(r.warnings);
				(gl as WebGLRenderer)
					.compileAsync(model.content, camera, scene)
					.catch(() => {})
					.finally(invalidate);
			},
			() => {},
		);
		return () => {
			aborter.abort();
			rig.current = null;
			r.dispose();
			invalidate();
		};
	}, [model, gl, camera, scene, invalidate, onWarnings]);

	React.useEffect(() => {
		rig.current?.setLook({ shading, outlines, wireframe });
		invalidate();
	}, [shading, outlines, wireframe, invalidate]);

	React.useEffect(() => {
		rig.current?.setEnabled(showWeapon, showProps);
		invalidate();
	}, [showWeapon, showProps, invalidate]);

	useFrame(() => {
		const r = rig.current;
		if (!r) return;
		const entry = controller?.currentEntry ?? null;
		const changed = r.update(
			entry?.file ?? null,
			entry?.source ?? null,
			controller?.time ?? 0,
		);
		if (changed) invalidate();
	});
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
	onPropWarnings,
	onContextLost,
	label,
}: StageProps): React.ReactElement {
	const controls = React.useRef<CameraControls | null>(null);
	const reframeRef = React.useRef<((transition: boolean) => void) | null>(null);
	const [framing, setFraming] = React.useState<Framing | null>(null);
	const [shown, setShown] = React.useState<LoadedModel | null>(null);
	const preset = LIGHTING[prefs.lighting];
	const toon = prefs.shading === "toon";
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
			camera={{
				fov: CAMERA_FOV,
				near: 0.01,
				far: 500,
				position: [0, 1.2, 4.2],
			}}
			className="touch-none"
			dpr={[1, prefs.dprCap]}
			frameloop="demand"
			gl={{
				antialias: true,
				alpha: true,
				stencil: true,
				powerPreference: "high-performance",
			}}
			onCreated={({ gl }) => {
				gl.toneMapping = NoToneMapping;
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
			<RendererSettings exposure={prefs.exposure} toon={toon} />
			<AdaptivePixelRatio cap={prefs.dprCap} />
			{prefs.stats && <StatsProbe />}
			<LightRig preset={preset} radius={radius} toon={toon} />
			<StudioEnvironment key={prefs.lighting} preset={preset} />
			<ShadingSync preset={preset} />
			<NprSync
				extraLights={prefs.extraLights}
				follow={prefs.lightFollow}
				groundShadow={prefs.groundShadow}
				model={shown}
				selfShadow={toon && prefs.selfShadow}
				stageFactor={background.stage && background.dark ? 0.5 : 1}
			/>
			{background.stage && (
				<StageBackdrop dark={background.dark} floor={prefs.floor} />
			)}
			<ModelMount
				controls={controls}
				hiddenParts={hiddenParts}
				model={model}
				onFramed={setFraming}
				onShown={handleShown}
				outlines={prefs.outlines}
				reframeRef={reframeRef}
				shading={prefs.shading}
				wireframe={prefs.wireframe}
				zUpMode={zUpMode}
			/>
			<CharProps
				controller={animation}
				model={shown}
				onWarnings={onPropWarnings}
				outlines={prefs.outlines}
				shading={prefs.shading}
				showProps={prefs.showProps}
				showWeapon={prefs.showWeapon}
				wireframe={prefs.wireframe}
			/>
			{prefs.floor && !background.stage && shown && framing && (
				<FloorDisc
					color={background.floor.color}
					opacity={background.floor.opacity}
					radius={footprint}
				/>
			)}
			{shown?.rigged && prefs.skeleton && <Skeleton model={shown} />}
			{prefs.groundShadow && shown?.rigged && framing && (
				<ContactBlobs placementKey={framing.version} root={shown.content} />
			)}
			{prefs.groundShadow && shown && !shown.rigged && framing && (
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
			<PanModifiers controls={controls} />
			<AutoRotate controls={controls} enabled={prefs.autoRotate && !locked} />
			<MixerDriver controller={animation} />
			<Bloom enabled={toon && prefs.bloom} />
			<HandleBridge
				background={background}
				controls={controls}
				handleRef={handleRef}
				reframeRef={reframeRef}
			/>
		</Canvas>
	);
}
