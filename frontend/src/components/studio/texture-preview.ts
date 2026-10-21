/**
 * Pixel access to loaded textures for the inspector: thumbnails drawn from
 * the texture's image, and exact RGBA bytes for the channel viewer (read
 * back through WebGL so alpha is never premultiplied into the colour data of
 * masks, ramps and packed maps).
 */
import { RGBAFormat, type Texture, UnsignedByteType } from "three";

export type Channel = "rgb" | "r" | "g" | "b" | "a";

export const CHANNELS: Channel[] = ["rgb", "r", "g", "b", "a"];

export const CHANNEL_LABEL: Record<Channel, string> = {
	rgb: "RGB",
	r: "R",
	g: "G",
	b: "B",
	a: "A",
};

export interface Pixels {
	width: number;
	height: number;
	/** Tightly packed RGBA, top row first. */
	data: Uint8ClampedArray<ArrayBuffer>;
}

/** Pixel bytes as the canvas APIs accept them. */
export type PixelBytes = Uint8ClampedArray<ArrayBuffer>;

/**
 * One channel as opaque greyscale (or the colour with alpha forced opaque),
 * so data stored in a single channel reads at full contrast.
 */
export function channelView(
	rgba: Uint8ClampedArray,
	channel: Channel,
): PixelBytes {
	const out = new Uint8ClampedArray(rgba.length) as PixelBytes;
	const offset =
		channel === "r" ? 0 : channel === "g" ? 1 : channel === "b" ? 2 : 3;
	for (let i = 0; i < rgba.length; i += 4) {
		if (channel === "rgb") {
			out[i] = rgba[i];
			out[i + 1] = rgba[i + 1];
			out[i + 2] = rgba[i + 2];
		} else {
			const v = rgba[i + offset];
			out[i] = v;
			out[i + 1] = v;
			out[i + 2] = v;
		}
		out[i + 3] = 255;
	}
	return out;
}

/** Which channels carry any information (not constant across the image). */
export function usedChannels(rgba: Uint8ClampedArray): Set<Channel> {
	const used = new Set<Channel>();
	const names: Channel[] = ["r", "g", "b", "a"];
	for (let c = 0; c < 4; c++) {
		const first = rgba[c];
		for (let i = c + 4; i < rgba.length; i += 4) {
			if (rgba[i] !== first) {
				used.add(names[c]);
				break;
			}
		}
	}
	return used;
}

/** Index of the pixel under a point given in texture pixels. */
export function pixelAt(
	pixels: Pixels,
	x: number,
	y: number,
): [number, number, number, number] | null {
	const px = Math.floor(x);
	const py = Math.floor(y);
	if (px < 0 || py < 0 || px >= pixels.width || py >= pixels.height) {
		return null;
	}
	const i = (py * pixels.width + px) * 4;
	const d = pixels.data;
	return [d[i], d[i + 1], d[i + 2], d[i + 3]];
}

type Drawable =
	| ImageBitmap
	| HTMLImageElement
	| HTMLCanvasElement
	| OffscreenCanvas
	| HTMLVideoElement
	| VideoFrame;

interface DataImage {
	data: ArrayBufferView;
	width: number;
	height: number;
}

function isDataImage(image: unknown): image is DataImage {
	const d = image as DataImage | null;
	return Boolean(
		d && typeof d === "object" && "data" in d && d.width > 0 && d.height > 0,
	);
}

function isDrawable(image: unknown): image is Drawable {
	if (!image || typeof image !== "object") return false;
	return (
		(typeof ImageBitmap !== "undefined" && image instanceof ImageBitmap) ||
		(typeof HTMLImageElement !== "undefined" &&
			image instanceof HTMLImageElement) ||
		(typeof HTMLCanvasElement !== "undefined" &&
			image instanceof HTMLCanvasElement) ||
		(typeof OffscreenCanvas !== "undefined" &&
			image instanceof OffscreenCanvas) ||
		(typeof HTMLVideoElement !== "undefined" &&
			image instanceof HTMLVideoElement)
	);
}

/** Pixels of a DataTexture, when it is plain 8-bit RGBA. */
function dataPixels(texture: Texture, image: DataImage): Pixels | null {
	if (texture.format !== RGBAFormat || texture.type !== UnsignedByteType) {
		return null;
	}
	const view = image.data;
	const bytes = new Uint8ClampedArray(
		view.buffer,
		view.byteOffset,
		view.byteLength,
	);
	if (bytes.length !== image.width * image.height * 4) return null;
	return { width: image.width, height: image.height, data: bytes.slice() };
}

let readbackGl: WebGL2RenderingContext | null | undefined;

/** A shared tiny WebGL2 context for texture readback (null when unavailable). */
function readbackContext(): WebGL2RenderingContext | null {
	if (readbackGl !== undefined) return readbackGl;
	try {
		const canvas = document.createElement("canvas");
		canvas.width = canvas.height = 1;
		readbackGl = canvas.getContext("webgl2", {
			premultipliedAlpha: false,
			preserveDrawingBuffer: false,
			antialias: false,
			depth: false,
			stencil: false,
		});
	} catch {
		readbackGl = null;
	}
	return readbackGl ?? null;
}

/**
 * Exact RGBA bytes of a drawable image: uploaded straight (no premultiply,
 * no colour-space conversion, no flip) and read back from a framebuffer.
 */
function glPixels(image: Drawable): Pixels | null {
	const gl = readbackContext();
	if (!gl) return null;
	const width =
		"videoWidth" in image
			? image.videoWidth
			: "displayWidth" in image
				? image.displayWidth
				: image.width;
	const height =
		"videoHeight" in image
			? image.videoHeight
			: "displayHeight" in image
				? image.displayHeight
				: image.height;
	if (
		!width ||
		!height ||
		width > gl.getParameter(gl.MAX_TEXTURE_SIZE) ||
		height > gl.getParameter(gl.MAX_TEXTURE_SIZE)
	) {
		return null;
	}
	const texture = gl.createTexture();
	const fbo = gl.createFramebuffer();
	try {
		gl.bindTexture(gl.TEXTURE_2D, texture);
		gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
		gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
		gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
		gl.texImage2D(
			gl.TEXTURE_2D,
			0,
			gl.RGBA,
			gl.RGBA,
			gl.UNSIGNED_BYTE,
			image as TexImageSource,
		);
		gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
		gl.framebufferTexture2D(
			gl.FRAMEBUFFER,
			gl.COLOR_ATTACHMENT0,
			gl.TEXTURE_2D,
			texture,
			0,
		);
		if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
			return null;
		}
		const data = new Uint8ClampedArray(width * height * 4);
		gl.readPixels(
			0,
			0,
			width,
			height,
			gl.RGBA,
			gl.UNSIGNED_BYTE,
			new Uint8Array(data.buffer),
		);
		if (gl.getError() !== gl.NO_ERROR) return null;
		return { width, height, data };
	} catch {
		return null;
	} finally {
		gl.bindFramebuffer(gl.FRAMEBUFFER, null);
		gl.bindTexture(gl.TEXTURE_2D, null);
		gl.deleteFramebuffer(fbo);
		gl.deleteTexture(texture);
	}
}

/** Canvas 2D fallback: alpha gets premultiplied, so only used without WebGL. */
function canvasPixels(image: Drawable): Pixels | null {
	const width = "width" in image ? Number(image.width) : 0;
	const height = "height" in image ? Number(image.height) : 0;
	if (!width || !height) return null;
	const canvas = document.createElement("canvas");
	canvas.width = width;
	canvas.height = height;
	const ctx = canvas.getContext("2d", { willReadFrequently: true });
	if (!ctx) return null;
	ctx.drawImage(image as CanvasImageSource, 0, 0);
	const img = ctx.getImageData(0, 0, width, height);
	return { width, height, data: img.data as PixelBytes };
}

const pixelCache = new WeakMap<Texture, Pixels | null>();

/** RGBA bytes of a texture, top row first; cached per texture. Null when unreadable. */
export function readTexturePixels(texture: Texture): Pixels | null {
	if (pixelCache.has(texture)) return pixelCache.get(texture) ?? null;
	const image: unknown = texture.image;
	let pixels: Pixels | null = null;
	if (isDataImage(image)) pixels = dataPixels(texture, image);
	else if (isDrawable(image)) pixels = glPixels(image) ?? canvasPixels(image);
	pixelCache.set(texture, pixels);
	return pixels;
}

/**
 * Draws a texture scaled into a small canvas (contain-fit). Cheap: bitmap
 * images go through drawImage; data textures through an ImageData.
 */
export function drawThumbnail(
	texture: Texture,
	canvas: HTMLCanvasElement,
	size: number,
): boolean {
	const ctx = canvas.getContext("2d");
	if (!ctx) return false;
	const image: unknown = texture.image;
	let source: CanvasImageSource | null = null;
	let width = 0;
	let height = 0;
	if (isDrawable(image)) {
		source = image as CanvasImageSource;
		width = "width" in image ? Number(image.width) : 0;
		height = "height" in image ? Number(image.height) : 0;
	} else if (isDataImage(image)) {
		const pixels = dataPixels(texture, image);
		if (!pixels) return false;
		const full = document.createElement("canvas");
		full.width = pixels.width;
		full.height = pixels.height;
		full
			.getContext("2d")
			?.putImageData(
				new ImageData(pixels.data, pixels.width, pixels.height),
				0,
				0,
			);
		source = full;
		width = pixels.width;
		height = pixels.height;
	}
	if (!source || !width || !height) return false;
	const dpr = Math.min(window.devicePixelRatio || 1, 2);
	canvas.width = canvas.height = Math.round(size * dpr);
	const scale = Math.min(canvas.width / width, canvas.height / height);
	const w = Math.max(1, Math.round(width * scale));
	const h = Math.max(1, Math.round(height * scale));
	ctx.clearRect(0, 0, canvas.width, canvas.height);
	ctx.imageSmoothingEnabled = width > w;
	ctx.drawImage(source, (canvas.width - w) / 2, (canvas.height - h) / 2, w, h);
	return true;
}
