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
export const ZOOM_STEP = 0.8;

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
