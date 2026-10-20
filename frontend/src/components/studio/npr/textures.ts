/**
 * Procedural stand-ins for data textures. Samplers must be bound even when a
 * define disables their use, and ramps declared by a material but not yet
 * embedded by the exporter fall back to a soft two-tone ramp so the ramp
 * shading path still runs.
 */
import {
	ClampToEdgeWrapping,
	DataTexture,
	LinearFilter,
	RGBAFormat,
} from "three";

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

let ramp: DataTexture | null = null;

/**
 * Fallback diffuse ramp: white rgb (no tint) with the lit mask in alpha
 * rising smoothly across the middle of the N·L range.
 */
export function fallbackRamp(): DataTexture {
	if (ramp) return ramp;
	const width = 256;
	const data = new Uint8Array(width * 4);
	for (let i = 0; i < width; i++) {
		const u = (i + 0.5) / width;
		const t = Math.min(Math.max((u - 0.45) / 0.1, 0), 1);
		const a = t * t * (3 - 2 * t);
		data[i * 4] = 255;
		data[i * 4 + 1] = 255;
		data[i * 4 + 2] = 255;
		data[i * 4 + 3] = Math.round(a * 255);
	}
	ramp = new DataTexture(data, width, 1, RGBAFormat);
	ramp.minFilter = LinearFilter;
	ramp.magFilter = LinearFilter;
	ramp.wrapS = ClampToEdgeWrapping;
	ramp.wrapT = ClampToEdgeWrapping;
	ramp.generateMipmaps = false;
	ramp.needsUpdate = true;
	return ramp;
}
