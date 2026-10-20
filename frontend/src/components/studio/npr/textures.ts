/**
 * Procedural stand-ins for data textures. Samplers must be bound even when a
 * define disables their use, and ramps declared by a material but not yet
 * embedded by the exporter fall back to a per-kind ramp shaped like the
 * game's common `_RD` ramps, so the ramp shading path still runs and the
 * look degrades towards the real thing rather than to a hard two-tone.
 */
import {
	ClampToEdgeWrapping,
	DataTexture,
	LinearFilter,
	RGBAFormat,
} from "three";
import type { NprKind } from "@/components/studio/npr/extras";

let placeholder: DataTexture | null = null;

/** A 1×1 white texture for unused sampler uniforms. */
export function placeholderTexture(): DataTexture {
	if (placeholder) return placeholder;
	placeholder = new DataTexture(
		new Uint8Array([255, 255, 255, 255]),
		1,
		1,
		RGBAFormat,
	);
	placeholder.needsUpdate = true;
	return placeholder;
}

/** Keyframes `[u, r, g, b, a]` (0–255) of the common diffuse ramps: rgb is the tint, alpha the lit mask. */
type RampKey = [number, number, number, number, number];

const RAMP_KEYS: Record<NprKind, RampKey[]> = {
	// Skin: a wide, warm transition from a quarter to seven eighths.
	skin: [
		[0, 0, 0, 0, 0],
		[0.125, 0, 0, 0, 0],
		[0.25, 27, 18, 14, 20],
		[0.375, 104, 69, 53, 71],
		[0.5, 180, 120, 92, 122],
		[0.625, 250, 171, 135, 173],
		[0.75, 253, 226, 214, 224],
		[0.875, 255, 255, 255, 255],
		[1, 255, 255, 255, 255],
	],
	// Hair: dark until the middle, then a pink-tinted rise.
	hair: [
		[0, 0, 0, 0, 0],
		[0.5, 32, 32, 32, 0],
		[0.625, 203, 107, 107, 62],
		[0.75, 209, 185, 185, 126],
		[0.875, 246, 246, 246, 191],
		[1, 255, 255, 255, 255],
	],
	// Cloth: a cool blue-violet shadow, a warm edge, then white.
	cloth: [
		[0, 109, 60, 58, 0],
		[0.125, 70, 73, 108, 1],
		[0.25, 65, 68, 104, 3],
		[0.375, 60, 63, 99, 4],
		[0.5, 54, 58, 95, 6],
		[0.625, 213, 164, 144, 23],
		[0.75, 239, 227, 210, 125],
		[0.875, 247, 241, 232, 194],
		[1, 255, 255, 255, 255],
	],
};

const ramps = new Map<NprKind, DataTexture>();

/** Fallback diffuse ramp for a shader kind (256×1 RGBA, linearly interpolated keys). */
export function fallbackRamp(kind: NprKind): DataTexture {
	const cached = ramps.get(kind);
	if (cached) return cached;
	const keys = RAMP_KEYS[kind];
	const width = 256;
	const data = new Uint8Array(width * 4);
	let k = 0;
	for (let i = 0; i < width; i++) {
		const u = i / (width - 1);
		while (k < keys.length - 2 && u > keys[k + 1][0]) k++;
		const a = keys[k];
		const b = keys[k + 1];
		const t = Math.min(
			Math.max((u - a[0]) / Math.max(b[0] - a[0], 1e-6), 0),
			1,
		);
		for (let c = 0; c < 4; c++) {
			data[i * 4 + c] = Math.round(a[c + 1] + (b[c + 1] - a[c + 1]) * t);
		}
	}
	const ramp = new DataTexture(data, width, 1, RGBAFormat);
	ramp.minFilter = LinearFilter;
	ramp.magFilter = LinearFilter;
	ramp.wrapS = ClampToEdgeWrapping;
	ramp.wrapT = ClampToEdgeWrapping;
	ramp.generateMipmaps = false;
	ramp.needsUpdate = true;
	ramps.set(kind, ramp);
	return ramp;
}

/** Grey keys `[u, v]` of the common hair spec ramp along N·H: a band near 0.5 that tails off. */
const HAIR_SPEC_KEYS: [number, number][] = [
	[0, 2],
	[0.125, 8],
	[0.25, 14],
	[0.375, 40],
	[0.5, 153],
	[0.625, 142],
	[0.75, 130],
	[0.875, 118],
	[1, 106],
];

let hairSpec: DataTexture | null = null;

/** Fallback hair spec ramp (256×1, sampled at v = 0.5 by the hair shader). */
export function fallbackHairSpecRamp(): DataTexture {
	if (hairSpec) return hairSpec;
	const width = 256;
	const data = new Uint8Array(width * 4);
	let k = 0;
	for (let i = 0; i < width; i++) {
		const u = i / (width - 1);
		while (k < HAIR_SPEC_KEYS.length - 2 && u > HAIR_SPEC_KEYS[k + 1][0]) k++;
		const [u0, v0] = HAIR_SPEC_KEYS[k];
		const [u1, v1] = HAIR_SPEC_KEYS[k + 1];
		const t = Math.min(Math.max((u - u0) / Math.max(u1 - u0, 1e-6), 0), 1);
		const v = Math.round(v0 + (v1 - v0) * t);
		data[i * 4] = v;
		data[i * 4 + 1] = v;
		data[i * 4 + 2] = v;
		data[i * 4 + 3] = 255;
	}
	hairSpec = new DataTexture(data, width, 1, RGBAFormat);
	hairSpec.minFilter = LinearFilter;
	hairSpec.magFilter = LinearFilter;
	hairSpec.wrapS = ClampToEdgeWrapping;
	hairSpec.wrapT = ClampToEdgeWrapping;
	hairSpec.generateMipmaps = false;
	hairSpec.needsUpdate = true;
	return hairSpec;
}
