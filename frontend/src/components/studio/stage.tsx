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
	AxesHelper,
	Box3,
	Box3Helper,
	type Camera,
	CanvasTexture,
	Color,
	type Group,
	type Intersection,
	type Mesh,
	NeutralToneMapping,
	NoToneMapping,
	type PerspectiveCamera,
	type Quaternion,
	Raycaster,
	SkeletonHelper,
	type SkinnedMesh,
	setConsoleFunction,
	Vector2,
	Vector3,
	type WebGLRenderer,
} from "three";
import type { AnimationController } from "@/components/studio/animation";
import { ContactShadows } from "@/components/studio/contact-shadows";
import {
	makeOverlay,
	NormalsHelper,
	selectionMaterials,
	wireOverlayColor,
} from "@/components/studio/helpers";
import { partOf } from "@/components/studio/inspect";
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
import type { StudioPrefs, ZUpMode } from "@/components/studio/prefs";
import {
	type DirLight,
	LIGHTING,
	type LightingPreset,
	type ResolvedBackground,
} from "@/components/studio/presets";
import { PropsRig } from "@/components/studio/props-rig";
import {
	applyShading,
	meshesOf,
	type ShadingLook,
	shadingUniforms,
	updateFaceFrames,
} from "@/components/studio/shading";
import { publishStats } from "@/components/studio/stats-store";
import {
	clampFov,
	distanceForFov,
	distanceFromZoom,
	distanceLimits,
	dollyTargetTowards,
	nearPlaneFor,
	presetDirection,
	type ViewPresetId,
	wheelZoomFactor,
} from "@/components/studio/view-math";
import {
	publishCamera,
	type Selection,
	type ViewState,
} from "@/components/studio/view-state";
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
	/** Multiplies the camera distance (< 1 zooms in). */
	zoomBy: (factor: number) => void;
	/** Slider position, 0 = far limit, 1 = near limit. */
	setZoom: (zoom: number) => void;
	/** Frames the whole model from a preset direction, keeping the lens. */
	setView: (preset: ViewPresetId) => void;
	/** Vertical field of view; the camera dollies so the subject keeps its size. */
	setFov: (fov: number) => void;
	/**
	 * Frames one mesh of the shown model (posed bounds for skinned meshes)
	 * from the current direction and orbits around it. False if not found.
	 */
	focusOn: (meshId: string) => boolean;
	screenshot: (filename: string, transparent?: boolean) => Promise<void>;
}

export interface StageProps {
	model: LoadedModel | null;
	zUpMode: ZUpMode;
	hiddenParts: ReadonlySet<string>;
	prefs: StudioPrefs;
	background: ResolvedBackground;
	locked: boolean;
	view: ViewState;
	/** A mesh was picked (or empty space, with null). */
	onPick: (selection: Selection | null) => void;
	/** A mesh was double-clicked: select it and frame the camera on it. */
	onFocus: (selection: Selection) => void;
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

/** Keeps the fallback toon rim, outline and wire-overlay uniforms in sync with the view. */
function ShadingSync({
	preset,
	dark,
}: {
	preset: LightingPreset;
	dark: boolean;
}) {
	const gl = useThree((s) => s.gl);
	const invalidate = useThree((s) => s.invalidate);
	const size = React.useMemo(() => new Vector2(), []);
	React.useEffect(() => {
		shadingUniforms.rimColor.value.set(preset.toon.rim.color);
		shadingUniforms.rimStrength.value = preset.toon.rim.strength;
		invalidate();
	}, [preset, invalidate]);
	React.useEffect(() => {
		wireOverlayColor.set(dark ? "#d8d2ff" : "#2a2440");
		invalidate();
	}, [dark, invalidate]);
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
interface ReframeOptions {
	transition: boolean;
	/**
	 * A view preset: world-space direction from the model to the camera. The
	 * current lens is kept and the saved (default) state left alone.
	 */
	direction?: Vec3;
}

function frameCamera(
	controls: CameraControls,
	camera: PerspectiveCamera,
	framing: Framing,
	model: LoadedModel,
	orientation: Quaternion,
	viewport: { width: number; height: number },
	{ transition, direction }: ReframeOptions,
): void {
	const game = model.charscreen?.camera ?? null;
	const dir = direction
		? new Vector3(...direction)
		: new Vector3(
				...(game
					? cameraDirection(game)
					: model.rigged
						? FRONT_DIRECTION
						: STATIC_DIRECTION),
			).applyQuaternion(orientation);
	const fov = direction ? camera.fov : game ? game.fov : CAMERA_FOV;
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
	// Depth range sized to the model (and the stage dome); the near plane
	// then follows the camera in (see CameraSync) so close-ups never clip.
	camera.near = nearPlaneFor(fit.distance, radius);
	camera.far = Math.max(radius * 40, 60);
	camera.updateProjectionMatrix();
	const limits = distanceLimits(radius);
	controls.minDistance = limits.min;
	controls.maxDistance = limits.max;
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
	if (!direction) controls.saveState();
}

/** Room around a focused mesh. */
const FOCUS_MARGIN = 1.25;

/**
 * World-space bounds of one mesh as it draws right now: skinned meshes are
 * measured through their current pose, not their bind-space geometry.
 */
function meshWorldBox(mesh: Mesh): Box3 | null {
	const skinned = mesh as SkinnedMesh;
	mesh.updateWorldMatrix(true, false);
	if (skinned.isSkinnedMesh) {
		skinned.skeleton.update();
		skinned.computeBoundingBox();
		if (!skinned.boundingBox) return null;
		return skinned.boundingBox.clone().applyMatrix4(mesh.matrixWorld);
	}
	const g = mesh.geometry;
	if (!g.boundingBox) g.computeBoundingBox();
	if (!g.boundingBox) return null;
	return g.boundingBox.clone().applyMatrix4(mesh.matrixWorld);
}

/**
 * Frames one mesh from where the camera already is, keeping the lens, and
 * makes its centre the orbit pivot. The default view (Reset) is untouched.
 */
function focusMesh(
	controls: CameraControls,
	camera: PerspectiveCamera,
	mesh: Mesh,
	viewport: { width: number; height: number },
	transport: boolean,
): boolean {
	const box = meshWorldBox(mesh);
	if (!box || box.isEmpty()) return false;
	const target = controls.getTarget(new Vector3());
	const position = controls.getPosition(new Vector3());
	const dir = position.sub(target);
	if (dir.lengthSq() < 1e-12) dir.set(0, 0, 1);
	dir.normalize();
	const fit = fitBox({
		min: box.min.toArray() as Vec3,
		max: box.max.toArray() as Vec3,
		direction: dir.toArray() as Vec3,
		fov: camera.fov,
		aspect: viewport.width / Math.max(viewport.height, 1),
		insets: insetsFor(viewport.width, viewport.height, transport),
		margin: FOCUS_MARGIN,
	});
	// Tiny meshes (eyes, buckles) still stop at the dolly limit.
	const distance = Math.max(fit.distance, controls.minDistance);
	void controls.setLookAt(
		fit.target[0] + dir.x * distance,
		fit.target[1] + dir.y * distance,
		fit.target[2] + dir.z * distance,
		fit.target[0],
		fit.target[1],
		fit.target[2],
		true,
	);
	return true;
}

type Reframe = (options: ReframeOptions) => void;

interface MountProps {
	model: LoadedModel | null;
	zUpMode: ZUpMode;
	hiddenParts: ReadonlySet<string>;
	look: ShadingLook;
	onFramed: (framing: Framing | null) => void;
	onShown: (model: LoadedModel | null) => void;
	controls: React.RefObject<CameraControls | null>;
	/** Receives a function that re-frames the shown model (Reset, presets). */
	reframeRef: React.RefObject<Reframe | null>;
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
	look,
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
	const lookRef = React.useRef(look);
	lookRef.current = look;

	// Pre-compile shaders off-screen (with the final materials), then swap
	// the new model in.
	React.useEffect(() => {
		if (model === current) return;
		if (!model) {
			setCurrent(null);
			return;
		}
		let cancelled = false;
		applyShading(model.content, lookRef.current);
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
		const reframe: Reframe = (options) => {
			const c = controls.current;
			if (c) {
				frameCamera(
					c,
					camera,
					framing,
					current,
					t.quaternion,
					sizeRef.current,
					options,
				);
			}
		};
		reframeRef.current = reframe;
		// A new model snaps into place behind a fade; re-orienting the same
		// model glides.
		reframe({ transition: !swapped });
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
		applyShading(current.content, look);
		invalidate();
	}, [current, look, invalidate]);

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

// ------------------------------------------------------------- inspect

/** Whether every ancestor up to the scene is visible. */
function effectivelyVisible(object: Mesh): boolean {
	for (let o: Mesh["parent"] = object; o; o = o.parent) {
		if (!o.visible) return false;
	}
	return true;
}

/** The model's drawn meshes, for picking and the normals helper. */
function visibleMeshes(model: LoadedModel): Mesh[] {
	return meshesOf(model.content).filter(effectivelyVisible);
}

/**
 * The first visible mesh of the model under a client-space point (skinned
 * meshes in their current pose), or null for empty space.
 */
function raycastModel(
	raycaster: Raycaster,
	camera: Camera,
	el: HTMLElement,
	model: LoadedModel,
	clientX: number,
	clientY: number,
): Intersection | null {
	const rect = el.getBoundingClientRect();
	raycaster.setFromCamera(
		new Vector2(
			((clientX - rect.left) / Math.max(rect.width, 1)) * 2 - 1,
			-((clientY - rect.top) / Math.max(rect.height, 1)) * 2 + 1,
		),
		camera,
	);
	const meshes = visibleMeshes(model);
	for (const mesh of meshes) {
		const skinned = mesh as SkinnedMesh;
		if (skinned.isSkinnedMesh) {
			// Pose-dependent bounds gate the per-triangle test.
			skinned.computeBoundingBox();
			skinned.computeBoundingSphere();
		}
	}
	return raycaster.intersectObjects(meshes, false)[0] ?? null;
}

/**
 * Click-to-select: a press that neither travels nor lingers raycasts the
 * shown model (skinned meshes in their current pose) and reports the mesh
 * under the pointer, or null for empty space.
 */
function Picker({
	model,
	enabled,
	onPick,
	onFocus,
}: {
	model: LoadedModel | null;
	enabled: boolean;
	onPick: (selection: Selection | null) => void;
	onFocus: (selection: Selection) => void;
}) {
	const gl = useThree((s) => s.gl);
	const camera = useThree((s) => s.camera);
	const raycaster = React.useMemo(() => new Raycaster(), []);
	const pick = React.useCallback(
		(clientX: number, clientY: number): Selection | null => {
			if (!model) return null;
			const hit = raycastModel(
				raycaster,
				camera,
				gl.domElement,
				model,
				clientX,
				clientY,
			);
			if (!hit) return null;
			const mesh = hit.object as Mesh;
			return {
				partId: partOf(model, mesh)?.id ?? null,
				meshId: mesh.uuid,
				materialIndex: hit.face?.materialIndex ?? 0,
			};
		},
		[gl, camera, raycaster, model],
	);
	// Double-click frames a mesh whether or not pick mode is on.
	React.useEffect(() => {
		if (!model) return;
		const el = gl.domElement;
		const onDouble = (e: MouseEvent) => {
			if (e.button !== 0) return;
			const selection = pick(e.clientX, e.clientY);
			if (selection) onFocus(selection);
		};
		el.addEventListener("dblclick", onDouble);
		return () => el.removeEventListener("dblclick", onDouble);
	}, [gl, model, pick, onFocus]);
	React.useEffect(() => {
		if (!enabled || !model) return;
		const el = gl.domElement;
		let down: { x: number; y: number; t: number; id: number } | null = null;
		const onDown = (e: PointerEvent) => {
			if (e.button !== 0) return;
			down = {
				x: e.clientX,
				y: e.clientY,
				t: performance.now(),
				id: e.pointerId,
			};
		};
		const onUp = (e: PointerEvent) => {
			const d = down;
			down = null;
			if (!d || d.id !== e.pointerId) return;
			if (
				Math.hypot(e.clientX - d.x, e.clientY - d.y) > 6 ||
				performance.now() - d.t > 500
			) {
				return;
			}
			onPick(pick(e.clientX, e.clientY));
		};
		const onCancel = () => {
			down = null;
		};
		el.addEventListener("pointerdown", onDown);
		el.addEventListener("pointerup", onUp);
		el.addEventListener("pointercancel", onCancel);
		return () => {
			el.removeEventListener("pointerdown", onDown);
			el.removeEventListener("pointerup", onUp);
			el.removeEventListener("pointercancel", onCancel);
		};
	}, [gl, model, enabled, onPick, pick]);
	return null;
}

/** Translucent tint and wire over the picked mesh. */
function SelectionHighlight({
	model,
	selection,
}: {
	model: LoadedModel;
	selection: Selection;
}) {
	const invalidate = useThree((s) => s.invalidate);
	React.useEffect(() => {
		const mesh = model.content.getObjectByProperty("uuid", selection.meshId) as
			| Mesh
			| undefined;
		if (!mesh?.isMesh) return;
		const [tint, wire] = selectionMaterials();
		const overlays = [makeOverlay(mesh, tint), makeOverlay(mesh, wire)];
		for (const o of overlays) {
			o.name = "selection";
			o.renderOrder = mesh.renderOrder + 2;
			mesh.add(o);
		}
		invalidate();
		return () => {
			for (const o of overlays) o.removeFromParent();
			tint.dispose();
			wire.dispose();
			invalidate();
		};
	}, [model, selection.meshId, invalidate]);
	return null;
}

/**
 * Vertex normals for the selected mesh, or every visible mesh under a
 * global cap. Built when toggled (and when the set of meshes changes); the
 * positions are refreshed only while a clip plays.
 */
function Normals({
	model,
	selection,
	visibilityKey,
	radius,
	animation,
}: {
	model: LoadedModel;
	selection: Selection | null;
	visibilityKey: string;
	radius: number;
	animation: AnimationController | null;
}) {
	const invalidate = useThree((s) => s.invalidate);
	const meshId = selection?.meshId ?? null;
	// biome-ignore lint/correctness/useExhaustiveDependencies: visibilityKey tracks part toggles
	const helper = React.useMemo(() => {
		const picked = meshId
			? (model.content.getObjectByProperty("uuid", meshId) as Mesh | undefined)
			: undefined;
		const meshes = picked?.isMesh ? [picked] : visibleMeshes(model);
		return new NormalsHelper(meshes, Math.max(radius * 0.025, 0.002));
	}, [model, meshId, visibilityKey, radius]);
	React.useEffect(() => {
		invalidate();
		return () => helper.dispose();
	}, [helper, invalidate]);
	useFrame(() => {
		if (animation?.state.playing) helper.update();
	});
	return <primitive object={helper.object} />;
}

function Bounds({ framing, dark }: { framing: Framing; dark: boolean }) {
	const helper = React.useMemo(() => {
		const box = new Box3(framing.min, framing.max);
		const h = new Box3Helper(box, new Color(dark ? "#f0ad4e" : "#c47d0e"));
		h.raycast = () => {};
		return h;
	}, [framing, dark]);
	React.useEffect(() => () => helper.dispose(), [helper]);
	return <primitive object={helper} />;
}

function Axes({ size }: { size: number }) {
	const helper = React.useMemo(() => {
		const h = new AxesHelper(size);
		h.raycast = () => {};
		h.renderOrder = 5;
		return h;
	}, [size]);
	React.useEffect(() => () => helper.dispose(), [helper]);
	return <primitive object={helper} position={[0, 0.001 * size, 0]} />;
}

/**
 * Publishes the camera distance, limits and lens for the View panel, and
 * slides the near plane in with the camera so a close-up of a face does not
 * clip through it.
 */
function CameraSync({
	controls,
	radius,
}: {
	controls: React.RefObject<CameraControls | null>;
	radius: number;
}) {
	useFrame(() => {
		const c = controls.current;
		if (!c) return;
		const camera = c.camera as PerspectiveCamera;
		const near = nearPlaneFor(c.distance, radius);
		if (Math.abs(near - camera.near) > camera.near * 0.05) {
			camera.near = near;
			camera.updateProjectionMatrix();
		}
		publishCamera({
			distance: c.distance,
			minDistance: c.minDistance,
			maxDistance: c.maxDistance,
			fov: camera.fov,
		});
	});
	return null;
}

/** The dolly end value camera-controls is damping towards. */
interface ControlsInternals {
	_sphericalEnd: { radius: number };
}

/** How often the surface under the pointer is re-probed during a gesture. */
const ANCHOR_PROBE_MS = 150;

/**
 * Wheel and trackpad zoom, replacing camera-controls' wheel handling: a
 * stronger, device-aware step (see `wheelZoomFactor`), pinch treated as a
 * dolly rather than a lens zoom, and the orbit target pulled towards what is
 * under the pointer so zooming goes where you look. The anchor is the model
 * surface under the pointer (probed a few times a second, since a raycast
 * through a posed character is not free), so the camera closes in on a face
 * and stops at the dolly limit instead of passing through the head; over
 * empty space it is the point on the plane through the target. Pinch on
 * touch screens stays with camera-controls.
 */
function WheelZoom({
	controls,
	model,
}: {
	controls: React.RefObject<CameraControls | null>;
	model: LoadedModel | null;
}) {
	const gl = useThree((s) => s.gl);
	const invalidate = useThree((s) => s.invalidate);
	const raycaster = React.useMemo(() => new Raycaster(), []);
	const modelRef = React.useRef(model);
	modelRef.current = model;
	React.useEffect(() => {
		const c = controls.current;
		if (!c) return;
		c.mouseButtons.wheel = CameraControlsImpl.ACTION.NONE;
		const el = gl.domElement;
		const target = new Vector3();
		const position = new Vector3();
		const right = new Vector3();
		const up = new Vector3();
		const cursor = new Vector3();
		const anchor = { point: new Vector3(), hit: false, x: 0, y: 0, at: 0 };
		const probe = (x: number, y: number, now: number): boolean => {
			const fresh =
				now - anchor.at < ANCHOR_PROBE_MS &&
				Math.hypot(x - anchor.x, y - anchor.y) < 6;
			if (fresh) return anchor.hit;
			anchor.at = now;
			anchor.x = x;
			anchor.y = y;
			const m = modelRef.current;
			const hit = m ? raycastModel(raycaster, c.camera, el, m, x, y) : null;
			anchor.hit = Boolean(hit);
			if (hit) anchor.point.copy(hit.point);
			return anchor.hit;
		};
		const onWheel = (e: WheelEvent) => {
			e.preventDefault();
			if (!c.enabled) return;
			const factor = wheelZoomFactor(e);
			if (factor === 1) return;
			const from = (c as unknown as ControlsInternals)._sphericalEnd.radius;
			let to = clampDistance(c, from * factor);
			c.getTarget(target);
			c.getPosition(position);
			if (probe(e.clientX, e.clientY, performance.now())) {
				cursor.copy(anchor.point);
				// Closing in scales the camera-to-surface gap with the distance;
				// keep that gap at least the dolly limit.
				const gap = position.distanceTo(cursor);
				if (gap > 0) to = Math.max(to, (from * c.minDistance) / gap);
				to = clampDistance(c, to);
			} else {
				// The point under the pointer on the plane through the target.
				const rect = el.getBoundingClientRect();
				const nx = ((e.clientX - rect.left) / Math.max(rect.width, 1)) * 2 - 1;
				const ny = -((e.clientY - rect.top) / Math.max(rect.height, 1)) * 2 + 1;
				const camera = c.camera as PerspectiveCamera;
				const tanV = Math.tan((camera.fov * Math.PI) / 360);
				const tanH = tanV * camera.aspect;
				right.setFromMatrixColumn(camera.matrixWorld, 0).normalize();
				up.setFromMatrixColumn(camera.matrixWorld, 1).normalize();
				const depth = position.distanceTo(target);
				cursor
					.copy(target)
					.addScaledVector(right, nx * tanH * depth)
					.addScaledVector(up, ny * tanV * depth);
			}
			if (to === from) return;
			const next = dollyTargetTowards(
				target.toArray() as Vec3,
				cursor.toArray() as Vec3,
				to / from,
			);
			void c.moveTo(next[0], next[1], next[2], true);
			void c.dollyTo(to, true);
			invalidate();
		};
		el.addEventListener("wheel", onWheel, { passive: false });
		return () => {
			el.removeEventListener("wheel", onWheel);
			c.mouseButtons.wheel = CameraControlsImpl.ACTION.DOLLY;
		};
	}, [gl, controls, invalidate, raycaster]);
	return null;
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

const clampDistance = (c: CameraControls, d: number) =>
	Math.min(c.maxDistance, Math.max(c.minDistance, d));

function HandleBridge({
	handleRef,
	controls,
	background,
	reframeRef,
	shown,
}: {
	handleRef: React.RefObject<StageHandle | null>;
	controls: React.RefObject<CameraControls | null>;
	background: ResolvedBackground;
	reframeRef: React.RefObject<Reframe | null>;
	shown: LoadedModel | null;
}) {
	const gl = useThree((s) => s.gl);
	const scene = useThree((s) => s.scene);
	const camera = useThree((s) => s.camera);
	const invalidate = useThree((s) => s.invalidate);
	const size = useThree((s) => s.size);
	const sizeRef = React.useRef(size);
	sizeRef.current = size;
	React.useImperativeHandle(
		handleRef,
		() => ({
			focusOn: (meshId) => {
				const c = controls.current;
				if (!c?.enabled || !shown) return false;
				const mesh = shown.content.getObjectByProperty("uuid", meshId) as
					| Mesh
					| undefined;
				if (!mesh?.isMesh) return false;
				const ok = focusMesh(
					c,
					camera as PerspectiveCamera,
					mesh,
					sizeRef.current,
					Boolean(shown.clips?.entries.length),
				);
				invalidate();
				return ok;
			},
			resetView: () => {
				const c = controls.current;
				if (!c) return;
				if (reframeRef.current) reframeRef.current({ transition: true });
				else void c.reset(true);
				invalidate();
			},
			zoomBy: (factor) => {
				const c = controls.current;
				if (!c?.enabled) return;
				void c.dollyTo(clampDistance(c, c.distance * factor), true);
				invalidate();
			},
			setZoom: (zoom) => {
				const c = controls.current;
				if (!c?.enabled) return;
				void c.dollyTo(
					distanceFromZoom(zoom, c.minDistance, c.maxDistance),
					false,
				);
				invalidate();
			},
			setView: (preset) => {
				const c = controls.current;
				if (!c?.enabled || !reframeRef.current) return;
				reframeRef.current({
					transition: true,
					direction: presetDirection(preset),
				});
				invalidate();
			},
			setFov: (fov) => {
				const c = controls.current;
				if (!c?.enabled) return;
				const cam = c.camera as PerspectiveCamera;
				const next = clampFov(fov);
				const distance = distanceForFov(c.distance, cam.fov, next);
				cam.fov = next;
				cam.updateProjectionMatrix();
				void c.dollyTo(clampDistance(c, distance), false);
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
			screenshot: async (filename: string, transparent = false) => {
				// The stage backdrop is scene geometry; a transparent capture
				// leaves it (and the painted background) out.
				const hidden: Mesh[] = [];
				if (transparent) {
					for (const child of scene.children) {
						const mesh = child as Mesh;
						const material = mesh.material as { name?: string } | undefined;
						if (
							mesh.isMesh &&
							mesh.visible &&
							material?.name?.startsWith("stage ")
						) {
							mesh.visible = false;
							hidden.push(mesh);
						}
					}
				}
				gl.render(scene, camera);
				for (const mesh of hidden) mesh.visible = true;
				if (hidden.length) invalidate();
				const src = gl.domElement;
				const out = document.createElement("canvas");
				out.width = src.width;
				out.height = src.height;
				const ctx = out.getContext("2d");
				if (!ctx) throw new Error("Canvas 2D is unavailable");
				if (!transparent) background.paint(ctx, out.width, out.height);
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
		[gl, scene, camera, controls, background, reframeRef, invalidate, shown],
	);
	return null;
}

/**
 * Pan mode swaps the primary gesture (left-drag, one finger) from orbit to
 * pan, which is what touch users need. Shift- or Ctrl-drag with the left
 * button pans too, for trackpads without a right button: the modifier is read
 * on pointerdown (before camera-controls sees it) and the button mapping
 * restored on release. Right-drag and two-finger drag pan through
 * camera-controls' defaults.
 */
function PanModifiers({
	controls,
	panMode,
}: {
	controls: React.RefObject<CameraControls | null>;
	panMode: boolean;
}) {
	const gl = useThree((s) => s.gl);
	const mode = React.useRef(panMode);
	mode.current = panMode;
	React.useEffect(() => {
		const c = controls.current;
		if (!c) return;
		c.mouseButtons.left = panMode
			? CameraControlsImpl.ACTION.TRUCK
			: CameraControlsImpl.ACTION.ROTATE;
		c.touches.one = panMode
			? CameraControlsImpl.ACTION.TOUCH_TRUCK
			: CameraControlsImpl.ACTION.TOUCH_ROTATE;
	}, [controls, panMode]);
	React.useEffect(() => {
		const el = gl.domElement;
		// The primary action to restore after a modifier drag (pan mode may
		// have changed meanwhile).
		const primary = () =>
			mode.current
				? CameraControlsImpl.ACTION.TRUCK
				: CameraControlsImpl.ACTION.ROTATE;
		const onDown = (e: PointerEvent) => {
			const c = controls.current;
			if (!c?.enabled || e.button !== 0 || !(e.shiftKey || e.ctrlKey)) return;
			c.mouseButtons.left = CameraControlsImpl.ACTION.TRUCK;
			const restore = () => {
				c.mouseButtons.left = primary();
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
	look,
	showWeapon,
	showProps,
	onWarnings,
}: {
	model: LoadedModel | null;
	controller: AnimationController | null;
	look: ShadingLook;
	showWeapon: boolean;
	showProps: boolean;
	onWarnings: (warnings: string[]) => void;
}) {
	const gl = useThree((s) => s.gl);
	const camera = useThree((s) => s.camera);
	const scene = useThree((s) => s.scene);
	const invalidate = useThree((s) => s.invalidate);
	const rig = React.useRef<PropsRig | null>(null);
	const lookRef = React.useRef(look);
	lookRef.current = look;
	const enabled = React.useRef({ showWeapon, showProps });
	enabled.current = { showWeapon, showProps };

	React.useEffect(() => {
		rig.current = null;
		const cs = model?.charscreen;
		if (!model || !cs || (!cs.weapon?.glb && !cs.deco?.items.length)) return;
		const r = new PropsRig(model);
		const aborter = new AbortController();
		void r.load(aborter.signal, lookRef.current).then(
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
		rig.current?.setLook(look);
		invalidate();
	}, [look, invalidate]);

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
	view,
	onPick,
	onFocus,
	animation,
	handleRef,
	onShown,
	onPropWarnings,
	onContextLost,
	label,
}: StageProps): React.ReactElement {
	const controls = React.useRef<CameraControls | null>(null);
	const reframeRef = React.useRef<Reframe | null>(null);
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
	const look = React.useMemo<ShadingLook>(
		() => ({
			shading: prefs.shading,
			outlines: prefs.outlines,
			wireframe: prefs.wireframe,
			wireframeOverlay: prefs.wireframeOverlay,
			surface: prefs.surface,
		}),
		[
			prefs.shading,
			prefs.outlines,
			prefs.wireframe,
			prefs.wireframeOverlay,
			prefs.surface,
		],
	);
	// A selection that no longer belongs to the shown model is ignored.
	const selection =
		view.selection &&
		shown?.content.getObjectByProperty("uuid", view.selection.meshId)
			? view.selection
			: null;

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
			<ShadingSync dark={background.dark} preset={preset} />
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
				look={look}
				model={model}
				onFramed={setFraming}
				onShown={handleShown}
				reframeRef={reframeRef}
				zUpMode={zUpMode}
			/>
			<CharProps
				controller={animation}
				look={look}
				model={shown}
				onWarnings={onPropWarnings}
				showProps={prefs.showProps}
				showWeapon={prefs.showWeapon}
			/>
			{prefs.floor && !background.stage && shown && framing && (
				<FloorDisc
					color={background.floor.color}
					opacity={background.floor.opacity}
					radius={footprint}
				/>
			)}
			{shown?.rigged && prefs.skeleton && <Skeleton model={shown} />}
			{shown && selection && (
				<SelectionHighlight model={shown} selection={selection} />
			)}
			{shown && view.normals && (
				<Normals
					animation={animation}
					model={shown}
					radius={radius}
					selection={selection}
					visibilityKey={visibilityKey}
				/>
			)}
			{shown && framing && prefs.bounds && (
				<Bounds dark={background.dark} framing={framing} />
			)}
			{shown && prefs.axes && <Axes size={Math.max(radius * 0.6, 0.1)} />}
			<Picker
				enabled={view.picking}
				model={shown}
				onFocus={onFocus}
				onPick={onPick}
			/>
			{prefs.groundShadow && shown?.rigged && framing && (
				<ContactBlobs placementKey={framing.version} root={shown.content} />
			)}
			{prefs.groundShadow && shown && !shown.rigged && framing && (
				<ContactShadows
					bakeKey={`${framing.version}:${visibilityKey}:${prefs.wireframe}:${prefs.shading}:${prefs.outlines}:${prefs.surface}`}
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
			{/* Touch pinch dollies through camera-controls; wheel and trackpad
			    go through WheelZoom. The near plane follows in CameraSync. */}
			<CameraControls
				dollySpeed={1}
				dollyToCursor
				makeDefault
				ref={controls}
				regress
				smoothTime={0.15}
			/>
			<ControlsLock controls={controls} locked={locked} />
			<PanModifiers controls={controls} panMode={view.panMode} />
			<WheelZoom controls={controls} model={shown} />
			<CameraSync controls={controls} radius={radius} />
			<AutoRotate controls={controls} enabled={prefs.autoRotate && !locked} />
			<MixerDriver controller={animation} />
			<Bloom enabled={toon && prefs.bloom} />
			<HandleBridge
				background={background}
				controls={controls}
				handleRef={handleRef}
				reframeRef={reframeRef}
				shown={shown}
			/>
		</Canvas>
	);
}
