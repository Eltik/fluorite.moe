/**
 * Typed access to the material `extras` our GLB exporter writes: the Unity
 * shader name, floats, colours, keywords and texture slots. GLTFLoader copies
 * extras onto `material.userData`; data textures the loader resolves for the
 * slots are stored under `userData.nprTextures` (see load-model).
 */
import { Color, type Material, SRGBColorSpace, type Texture } from "three";

/** Colours come as `{r,g,b,a}` objects (ours) or `[r,g,b,a]` arrays. */
export type ExtrasColor =
	| { r: number; g: number; b: number; a?: number }
	| number[];

export interface ExtrasTextureSlot {
	name?: string;
	/** glTF texture index, or null when the exporter did not embed it. */
	texture: number | null;
	uvSet?: number;
	scale?: { x: number; y: number };
	offset?: { x: number; y: number };
}

export interface MaterialExtras {
	shader: string;
	floats?: Record<string, number>;
	ints?: Record<string, number>;
	colors?: Record<string, ExtrasColor>;
	textures?: Record<string, ExtrasTextureSlot | number | null>;
	keywords?: string[];
	renderQueue?: number;
	/** Slots whose textures are colour data (absent for characters). */
	srgb?: string[];
}

export type NprKind = "skin" | "hair" | "cloth";

const NPR_KINDS: Record<string, NprKind> = {
	"HGRP/CharacterNPR_Skin": "skin",
	"HGRP/CharacterNPR_Hair": "hair",
	"HGRP/CharacterNPR": "cloth",
};

export const NPR_TEXTURES_KEY = "nprTextures";

export function extrasOf(material: Material): MaterialExtras | null {
	const data = material.userData as Partial<MaterialExtras>;
	return typeof data.shader === "string" ? (data as MaterialExtras) : null;
}

export function nprKind(shader: string | undefined): NprKind | null {
	return shader ? (NPR_KINDS[shader] ?? null) : null;
}

export function isEyeShader(shader: string | undefined): boolean {
	return shader === "HGRP/CharacterNPR_Eye";
}

export function isOverlayShadowShader(shader: string | undefined): boolean {
	return shader === "HGRP/CharacterNPR_OverlayShadow";
}

export function isVfxShader(shader: string | undefined): boolean {
	return (
		shader === "HGRP/CharacterNPR_VFX" ||
		(shader?.startsWith("HGRP/Effect/VFX") ?? false)
	);
}

export function floatOf(
	extras: MaterialExtras | null,
	name: string,
	fallback: number,
): number {
	const v = extras?.floats?.[name];
	return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

/** `floats[name] == 1` (Unity toggles are floats). */
export function flagOf(extras: MaterialExtras | null, name: string): boolean {
	return floatOf(extras, name, 0) === 1;
}

export function colorParts(
	value: ExtrasColor | undefined,
): [number, number, number, number] | null {
	if (!value) return null;
	if (Array.isArray(value)) {
		if (value.length < 3) return null;
		return [value[0], value[1], value[2], value[3] ?? 1];
	}
	return [value.r, value.g, value.b, value.a ?? 1];
}

/** A colour from `extras.colors`, converted sRGB → linear, or null. */
export function colorOf(
	extras: MaterialExtras | null,
	name: string,
): { color: Color; alpha: number } | null {
	const parts = colorParts(extras?.colors?.[name]);
	if (!parts) return null;
	return {
		color: new Color().setRGB(parts[0], parts[1], parts[2], SRGBColorSpace),
		alpha: parts[3],
	};
}

/** Raw (unconverted) colour components, for vectors stored as colours. */
export function vectorOf(
	extras: MaterialExtras | null,
	name: string,
): [number, number, number, number] | null {
	return colorParts(extras?.colors?.[name]);
}

/** glTF texture index of a slot, or null when absent / not embedded. */
export function textureIndexOf(
	extras: MaterialExtras | null,
	slot: string,
): number | null {
	const entry = extras?.textures?.[slot];
	if (typeof entry === "number") return entry;
	if (entry && typeof entry === "object" && typeof entry.texture === "number")
		return entry.texture;
	return null;
}

/** Whether the slot is declared at all (even without an embedded texture). */
export function hasSlot(extras: MaterialExtras | null, slot: string): boolean {
	return Boolean(extras?.textures && slot in extras.textures);
}

/** Data textures resolved for a material's slots. */
export function dataTexturesOf(material: Material): Record<string, Texture> {
	const t = material.userData[NPR_TEXTURES_KEY] as
		| Record<string, Texture>
		| undefined;
	return t ?? {};
}

export function dataTextureOf(
	material: Material,
	slot: string,
): Texture | null {
	return dataTexturesOf(material)[slot] ?? null;
}
