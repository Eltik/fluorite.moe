/**
 * Camera framing math for the Studio, free of three.js so it can be unit
 * tested: fits an axis-aligned box into the part of the viewport that the
 * interface leaves free, seen from a given direction with a given FOV.
 */

export type Vec3 = [number, number, number];

/** The game's character-screen camera, in model space (Y up). */
export interface GameCamera {
	position: Vec3;
	target: Vec3;
	/** Vertical field of view, degrees. */
	fov: number;
	near?: number;
	far?: number;
	/** Lens shift the game uses to push the character aside for its UI. */
	screenOffset?: [number, number];
	referenceAspect?: number;
}

/** Fractions of the viewport covered by interface chrome. */
export interface Insets {
	top: number;
	bottom: number;
	left: number;
	right: number;
}

export interface FitInput {
	min: Vec3;
	max: Vec3;
	/** Unit vector from the look-at target towards the camera. */
	direction: Vec3;
	/** Vertical field of view, degrees. */
	fov: number;
	/** Viewport width / height. */
	aspect: number;
	insets: Insets;
	/** Extra room around the box (1 = touching the free area). */
	margin: number;
}

export interface FitResult {
	target: Vec3;
	position: Vec3;
	distance: number;
}

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const add = (a: Vec3, b: Vec3): Vec3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const scale = (a: Vec3, s: number): Vec3 => [a[0] * s, a[1] * s, a[2] * s];
const dot = (a: Vec3, b: Vec3): number =>
	a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: Vec3, b: Vec3): Vec3 => [
	a[1] * b[2] - a[2] * b[1],
	a[2] * b[0] - a[0] * b[2],
	a[0] * b[1] - a[1] * b[0],
];
const length = (a: Vec3): number => Math.hypot(a[0], a[1], a[2]);

export function normalize(a: Vec3): Vec3 {
	const l = length(a);
	return l > 1e-9 ? scale(a, 1 / l) : [0, 0, 1];
}

/** `position - target`, normalised; straight ahead when they coincide. */
export function cameraDirection(camera: GameCamera): Vec3 {
	return normalize(sub(camera.position, camera.target));
}

/** Unit camera basis for a view along `-direction` with Y up. */
export function viewBasis(direction: Vec3): {
	forward: Vec3;
	right: Vec3;
	up: Vec3;
} {
	const forward = scale(normalize(direction), -1);
	let right = cross(forward, [0, 1, 0]);
	// Looking straight up or down: pick any horizontal right vector.
	if (length(right) < 1e-6) right = [1, 0, 0];
	right = normalize(right);
	const up = normalize(cross(right, forward));
	return { forward, right, up };
}

/**
 * Places the camera so the whole box fits inside the free part of the
 * viewport, centred in it, from `direction`. Box corners are projected onto
 * the camera's right / up axes, so the fit holds at any aspect ratio.
 */
export function fitBox(input: FitInput): FitResult {
	const { min, max, fov, aspect, insets, margin } = input;
	const direction = normalize(input.direction);
	const { forward, right, up } = viewBasis(direction);
	const center: Vec3 = [
		(min[0] + max[0]) / 2,
		(min[1] + max[1]) / 2,
		(min[2] + max[2]) / 2,
	];
	let halfW = 0;
	let halfH = 0;
	let halfD = 0;
	for (let i = 0; i < 8; i++) {
		const corner: Vec3 = [
			i & 1 ? max[0] : min[0],
			i & 2 ? max[1] : min[1],
			i & 4 ? max[2] : min[2],
		];
		const d = sub(corner, center);
		halfW = Math.max(halfW, Math.abs(dot(d, right)));
		halfH = Math.max(halfH, Math.abs(dot(d, up)));
		halfD = Math.max(halfD, Math.abs(dot(d, forward)));
	}
	const tanV = Math.tan((fov * Math.PI) / 360);
	const tanH = tanV * Math.max(aspect, 1e-3);
	const freeW = Math.max(1 - insets.left - insets.right, 0.2);
	const freeH = Math.max(1 - insets.top - insets.bottom, 0.2);
	const byHeight = halfH / (tanV * freeH);
	const byWidth = halfW / (tanH * freeW);
	// The near face sits `halfD` closer than the centre plane the projection
	// assumed, so push back by that much.
	const distance = Math.max(byHeight, byWidth) * margin + halfD;
	// The free area is usually off the viewport centre (more chrome at the
	// bottom), so aim such that the box centre lands in the free area's
	// middle: the look-at point shifts the opposite way.
	const visibleH = 2 * distance * tanV;
	const visibleW = visibleH * aspect;
	const offX = ((insets.left - insets.right) / 2) * visibleW;
	const offY = ((insets.bottom - insets.top) / 2) * visibleH;
	const target = sub(sub(center, scale(right, offX)), scale(up, offY));
	const position = add(target, scale(direction, distance));
	return { target, position, distance };
}

/** The default front view when a character has no game camera. */
export const FRONT_DIRECTION: Vec3 = normalize([0, 0.07, 1]);

/** A three-quarter view for static models (weapons, facilities). */
export const STATIC_DIRECTION: Vec3 = (() => {
	const polar = (80 * Math.PI) / 180;
	const azimuth = (28 * Math.PI) / 180;
	return normalize([
		Math.sin(polar) * Math.sin(azimuth),
		Math.cos(polar),
		Math.sin(polar) * Math.cos(azimuth),
	]);
})();
