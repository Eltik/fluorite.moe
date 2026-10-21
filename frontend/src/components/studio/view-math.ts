/**
 * Pure camera math for the View tools (no three.js, unit tested): the zoom
 * slider mapping, the view presets and the dolly-zoom that keeps a subject
 * the same size while the field of view changes.
 */
import { normalize, type Vec3 } from "@/lib/framing";

export const VIEW_PRESET_IDS = [
	"front",
	"back",
	"left",
	"right",
	"top",
	"threeQuarter",
] as const;
export type ViewPresetId = (typeof VIEW_PRESET_IDS)[number];

export const VIEW_PRESET_LABEL: Record<ViewPresetId, string> = {
	front: "Front",
	back: "Back",
	left: "Left",
	right: "Right",
	top: "Top",
	threeQuarter: "¾",
};

/** Keyboard digits 1–6, in the order the panel lists them. */
export const VIEW_PRESET_KEYS: Record<string, ViewPresetId> = {
	"1": "front",
	"2": "back",
	"3": "right",
	"4": "left",
	"5": "top",
	"6": "threeQuarter",
};

/**
 * Unit vector from the model towards the camera, in world space (the model
 * faces +Z once the stage has applied its Z-up correction). "Left" is the
 * model's left, as in Blender's numpad views. Top keeps a hair of Z so the
 * camera's up vector stays defined.
 */
export function presetDirection(id: ViewPresetId): Vec3 {
	switch (id) {
		case "front":
			return [0, 0, 1];
		case "back":
			return [0, 0, -1];
		case "left":
			return [-1, 0, 0];
		case "right":
			return [1, 0, 0];
		case "top":
			return normalize([0, 1, 0.001]);
		case "threeQuarter": {
			const polar = (72 * Math.PI) / 180;
			const azimuth = (35 * Math.PI) / 180;
			return normalize([
				Math.sin(polar) * Math.sin(azimuth),
				Math.cos(polar),
				Math.sin(polar) * Math.cos(azimuth),
			]);
		}
	}
}

const clamp01 = (t: number) => Math.min(1, Math.max(0, t));

/**
 * Slider position (0 = as far as allowed, 1 = as close as allowed) for a
 * camera distance, on a log scale so each step feels the same.
 */
export function zoomFromDistance(
	distance: number,
	min: number,
	max: number,
): number {
	if (!(min > 0) || !(max > min) || !Number.isFinite(distance)) return 0.5;
	const d = Math.min(max, Math.max(min, distance));
	return clamp01(1 - Math.log(d / min) / Math.log(max / min));
}

/** Inverse of {@link zoomFromDistance}. */
export function distanceFromZoom(
	zoom: number,
	min: number,
	max: number,
): number {
	if (!(min > 0) || !(max > min)) return min > 0 ? min : 1;
	return min * (max / min) ** (1 - clamp01(zoom));
}

/** Zoom step for the buttons and keys: a fixed ratio of the distance. */
export const ZOOM_STEP = 0.7;

/**
 * Dolly limits for a model of the given bounding radius: close enough to
 * fill the frame with a face or a buckle (2% of the radius, never under 2 cm
 * in model units), far enough to see the whole figure small.
 */
export function distanceLimits(radius: number): { min: number; max: number } {
	const r = Math.max(radius, 1e-3);
	const min = Math.max(r * 0.02, 0.02);
	return { min, max: Math.max(r * 14, min * 40) };
}

/**
 * Near plane for the current camera distance: a twentieth of the distance,
 * so close-ups never clip, bounded by the model's depth-precision budget.
 */
export function nearPlaneFor(distance: number, radius: number): number {
	const r = Math.max(radius, 1e-3);
	const near = Math.min(distance * 0.05, r * 0.01);
	return Math.max(near, r * 0.0005, 1e-4);
}

/** What a wheel event carries that matters for zooming. */
export interface WheelDelta {
	deltaY: number;
	/** 0 pixels, 1 lines, 2 pages. */
	deltaMode: number;
	/** macOS trackpad pinch arrives as a wheel with ctrlKey set. */
	ctrlKey: boolean;
}

/** One mouse-wheel notch in pixels (Chrome and Safari report 100 or so). */
const NOTCH_PX = 100;
/** Distance ratio per mouse-wheel notch. */
const NOTCH_RATIO = 0.62;
/**
 * Pinch deltas are small (a few px per event) and a doubling pinch sums to
 * about 70 px; three times the scroll gain makes that a 2.7× dolly.
 */
const PINCH_GAIN = 3;
/** Caps a single event (page scrolls, inertia bursts) to a sane step. */
const MAX_PX = 400;

/**
 * Multiplier for the camera distance from one wheel event (< 1 zooms in).
 * Scrolling up / pinching out zooms in. A notch is a clear step; trackpad
 * scrolls add many small steps that integrate to the same curve, and pinch
 * gets extra gain so one gesture sweeps most of the range.
 */
export function wheelZoomFactor(event: WheelDelta): number {
	let px = event.deltaY;
	if (event.deltaMode === 1) px *= 16;
	else if (event.deltaMode === 2) px *= 400;
	if (!Number.isFinite(px) || px === 0) return 1;
	px = Math.max(-MAX_PX, Math.min(MAX_PX, px));
	const gain = event.ctrlKey ? PINCH_GAIN : 1;
	return NOTCH_RATIO ** (-(px * gain) / NOTCH_PX);
}

/**
 * Where the orbit target moves so the point under the cursor stays put while
 * the distance scales by `scale`: towards that point when zooming in, away
 * from it when zooming out. `cursor` is the point on the plane through the
 * target facing the camera.
 */
export function dollyTargetTowards(
	target: Vec3,
	cursor: Vec3,
	scale: number,
): Vec3 {
	const k = 1 - scale;
	return [
		target[0] + (cursor[0] - target[0]) * k,
		target[1] + (cursor[1] - target[1]) * k,
		target[2] + (cursor[2] - target[2]) * k,
	];
}

/**
 * The camera distance that keeps a subject the same apparent size after a
 * change of vertical field of view (degrees).
 */
export function distanceForFov(
	distance: number,
	fromFov: number,
	toFov: number,
): number {
	const a = Math.tan((fromFov * Math.PI) / 360);
	const b = Math.tan((toFov * Math.PI) / 360);
	if (!(a > 0) || !(b > 0)) return distance;
	return (distance * a) / b;
}

export const FOV_MIN = 10;
export const FOV_MAX = 90;

export function clampFov(fov: number): number {
	return Math.min(FOV_MAX, Math.max(FOV_MIN, fov));
}

/** The next surface view after `current` (U cycles through them). */
export function nextSurface<T>(ids: readonly T[], current: T): T {
	const i = ids.indexOf(current);
	return ids[(i + 1) % ids.length];
}
