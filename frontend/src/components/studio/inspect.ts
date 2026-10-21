/**
 * Mesh inspection for the View panel: what a picked mesh is (name, material,
 * game shader, triangles, texture slots) and which parts entry it belongs
 * to. Reads the original materials the loader produced, not the shading
 * variants, so the game's shader names survive toon / debug modes.
 */
import {
	type BufferAttribute,
	DoubleSide,
	type InterleavedBufferAttribute,
	type Material,
	type Mesh,
	NoColorSpace,
	type Object3D,
	type SkinnedMesh,
	SRGBColorSpace,
	type Texture,
} from "three";
import type { LoadedModel, PartEntry } from "@/components/studio/load-model";
import {
	colorParts,
	dataTexturesOf,
	extrasOf,
	type MaterialExtras,
	textureIndexOf,
} from "@/components/studio/npr/extras";

export interface TextureSlot {
	/** Slot name as the game material names it (`_BaseMap`, …). */
	slot: string;
	/** Texture name or dimensions, when the texture is embedded. */
	detail: string | null;
}

export interface MeshInfo {
	name: string;
	/** Node path above the mesh, innermost last (empty for root meshes). */
	path: string[];
	materialName: string;
	/** The game's shader from the material extras, or null for plain GLBs. */
	shader: string | null;
	/** Material slot index on a multi-material mesh, else 0. */
	materialIndex: number;
	materialCount: number;
	triangles: number;
	vertices: number;
	skinned: boolean;
	textures: TextureSlot[];
}

/** Model-space triangle count of one geometry (indexed or not). */
export function triangleCountOf(mesh: Mesh): number {
	const g = mesh.geometry;
	const count = g.index ? g.index.count : (g.attributes.position?.count ?? 0);
	return Math.floor(count / 3);
}

function textureDetail(texture: Texture | null | undefined): string | null {
	if (!texture) return null;
	if (texture.name) return texture.name;
	const img = texture.image as
		| { width?: number; height?: number }
		| null
		| undefined;
	return img?.width && img?.height ? `${img.width}×${img.height}` : "embedded";
}

/** The material the loader produced, before any Studio shading variant. */
export function originalMaterial(mesh: Mesh, index: number): Material | null {
	const data = (
		mesh.userData as { studioShading?: { standard: Material | Material[] } }
	).studioShading;
	const source = data ? data.standard : mesh.material;
	if (Array.isArray(source)) return source[index] ?? source[0] ?? null;
	return source ?? null;
}

const STANDARD_MAPS: [string, string][] = [
	["map", "_BaseMap"],
	["normalMap", "_BumpMap"],
	["emissiveMap", "_EmissionMap"],
	["roughnessMap", "roughness"],
	["metalnessMap", "metalness"],
	["aoMap", "occlusion"],
	["alphaMap", "alpha"],
];

/** Texture slots of a material: game slots from the extras, else three's maps. */
export function textureSlotsOf(material: Material): TextureSlot[] {
	const extras = extrasOf(material);
	const slots: TextureSlot[] = [];
	const seen = new Set<string>();
	const standard = material as unknown as Record<string, Texture | null>;
	if (extras?.textures) {
		const data = dataTexturesOf(material);
		for (const slot of Object.keys(extras.textures)) {
			if (textureIndexOf(extras, slot) === null) continue;
			const bound =
				data[slot] ??
				(slot === "_BaseMap"
					? standard.map
					: slot === "_BumpMap"
						? standard.normalMap
						: slot === "_EmissionMap"
							? standard.emissiveMap
							: null);
			slots.push({ slot, detail: textureDetail(bound) });
			seen.add(slot);
		}
	}
	for (const [prop, label] of STANDARD_MAPS) {
		const texture = standard[prop];
		if (texture && !seen.has(label)) {
			slots.push({ slot: label, detail: textureDetail(texture) });
		}
	}
	return slots;
}

export function describeMesh(
	mesh: Mesh,
	materialIndex = 0,
	root?: Object3D,
): MeshInfo {
	const materials = Array.isArray(mesh.material)
		? mesh.material
		: [mesh.material];
	const index = Math.min(Math.max(materialIndex, 0), materials.length - 1);
	const material = originalMaterial(mesh, index);
	const path: string[] = [];
	for (let o = mesh.parent; o && o !== root; o = o.parent) {
		if (o.name) path.unshift(o.name);
	}
	return {
		name: mesh.name || "Unnamed mesh",
		path,
		materialName: material?.name || "Unnamed material",
		shader: material ? (extrasOf(material)?.shader ?? null) : null,
		materialIndex: index,
		materialCount: materials.length,
		triangles: triangleCountOf(mesh),
		vertices: mesh.geometry.attributes.position?.count ?? 0,
		skinned: Boolean((mesh as { isSkinnedMesh?: boolean }).isSkinnedMesh),
		textures: material ? textureSlotsOf(material) : [],
	};
}

/** The parts entry that owns an object (itself or an ancestor), or null. */
export function partOf(
	model: Pick<LoadedModel, "parts">,
	object: Object3D,
): PartEntry | null {
	const byObject = new Map(model.parts.map((p) => [p.object, p]));
	for (let o: Object3D | null = object; o; o = o.parent) {
		const part = byObject.get(o);
		if (part) return part;
	}
	return null;
}

/** Short display form of a game shader name (`HGRP/CharacterNPR_Skin` → `CharacterNPR_Skin`). */
export function shortShader(shader: string): string {
	const i = shader.lastIndexOf("/");
	return i >= 0 ? shader.slice(i + 1) : shader;
}

// ------------------------------------------------------------- detail

export type AlphaMode = "opaque" | "mask" | "blend";

export interface AttributeInfo {
	name: string;
	/** Components per vertex (3 for positions, 2 for UVs, 4 for bone weights). */
	size: number;
	/** Typed-array constructor name (`Float32Array`, `Uint16Array`, …). */
	type: string;
	normalized: boolean;
}

export interface GeometryDetail {
	vertices: number;
	triangles: number;
	indexed: boolean;
	/** Draw groups (glTF primitives merged into one geometry), else material count. */
	submeshes: number;
	attributes: AttributeInfo[];
	/** UV set attribute names present (`uv`, `uv1`, …). */
	uvSets: string[];
	skinned: boolean;
	/** Bones in the skeleton of a skinned mesh, else null. */
	bones: number | null;
}

export interface ColorDetail {
	name: string;
	/** Raw components as the exporter wrote them (unconverted). */
	rgba: [number, number, number, number];
	/** `#rrggbb` of the clamped RGB, for a swatch. */
	hex: string;
	/** Any component outside 0–1: an HDR colour or a vector in disguise. */
	hdr: boolean;
}

export interface MaterialDetail {
	name: string;
	shader: string | null;
	alphaMode: AlphaMode;
	/** Alpha cutoff of a masked material. */
	alphaCutoff: number | null;
	doubleSided: boolean;
	/** Unity render queue from the extras, else null. */
	renderQueue: number | null;
	floats: [string, number][];
	ints: [string, number][];
	colors: ColorDetail[];
	keywords: string[];
}

export type ColorSpaceLabel = "sRGB" | "Linear" | "Data";

export interface TextureDetail {
	slot: string;
	texture: Texture | null;
	name: string | null;
	width: number | null;
	height: number | null;
	colorSpace: ColorSpaceLabel | null;
	/** UV set the game samples this slot with, when known. */
	uvSet: number | null;
}

export interface MeshDetail {
	geometry: GeometryDetail;
	material: MaterialDetail;
	textures: TextureDetail[];
}

const UV_ATTRIBUTE = /^uv\d*$/;

type AnyAttribute = BufferAttribute | InterleavedBufferAttribute;

function attributeInfo(name: string, attr: AnyAttribute): AttributeInfo {
	const array = (attr as { array?: ArrayLike<number> }).array;
	return {
		name,
		size: attr.itemSize,
		type: array ? (array.constructor as { name: string }).name : "unknown",
		normalized: Boolean(attr.normalized),
	};
}

export function describeGeometry(mesh: Mesh): GeometryDetail {
	const g = mesh.geometry;
	const attributes = Object.entries(g.attributes).map(([name, attr]) =>
		attributeInfo(name, attr as AnyAttribute),
	);
	const materials = Array.isArray(mesh.material)
		? mesh.material.length
		: mesh.material
			? 1
			: 0;
	const skinned = mesh as SkinnedMesh;
	return {
		vertices: g.attributes.position?.count ?? 0,
		triangles: triangleCountOf(mesh),
		indexed: Boolean(g.index),
		submeshes: Math.max(g.groups.length, materials, 1),
		attributes,
		uvSets: attributes
			.map((a) => a.name)
			.filter((n) => UV_ATTRIBUTE.test(n))
			.sort(),
		skinned: Boolean(skinned.isSkinnedMesh),
		bones: skinned.isSkinnedMesh ? skinned.skeleton.bones.length : null,
	};
}

/**
 * glTF alpha mode as GLTFLoader expressed it on the material: BLEND sets
 * `transparent`, MASK sets `alphaTest` (hashed alpha counts as blend).
 */
export function alphaModeOf(material: Material): AlphaMode {
	const m = material as Material & { alphaHash?: boolean };
	if (m.transparent || m.alphaHash) return "blend";
	if (m.alphaTest > 0) return "mask";
	return "opaque";
}

const to255 = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 255);

/** `#rrggbb` for raw 0–1 components (clamped; no colour-space change). */
export function hexOf(rgb: [number, number, number]): string {
	return `#${rgb.map((c) => to255(c).toString(16).padStart(2, "0")).join("")}`;
}

function sortedEntries<T>(
	record: Record<string, T> | undefined,
): [string, T][] {
	return Object.entries(record ?? {}).sort(([a], [b]) => a.localeCompare(b));
}

export function describeMaterial(material: Material | null): MaterialDetail {
	const extras: MaterialExtras | null = material ? extrasOf(material) : null;
	const colors: ColorDetail[] = [];
	for (const [name, value] of sortedEntries(extras?.colors)) {
		const parts = colorParts(value);
		if (!parts) continue;
		colors.push({
			name,
			rgba: parts,
			hex: hexOf([parts[0], parts[1], parts[2]]),
			hdr: parts.some((c) => c < 0 || c > 1),
		});
	}
	return {
		name: material?.name || "Unnamed material",
		shader: extras?.shader ?? null,
		alphaMode: material ? alphaModeOf(material) : "opaque",
		alphaCutoff:
			material && alphaModeOf(material) === "mask" ? material.alphaTest : null,
		doubleSided: material?.side === DoubleSide,
		renderQueue:
			typeof extras?.renderQueue === "number" ? extras.renderQueue : null,
		floats: sortedEntries(extras?.floats).filter(
			([, v]) => typeof v === "number",
		),
		ints: sortedEntries(extras?.ints).filter(([, v]) => typeof v === "number"),
		colors,
		keywords: [...(extras?.keywords ?? [])].sort(),
	};
}

export function colorSpaceLabel(texture: Texture): ColorSpaceLabel {
	if (texture.colorSpace === SRGBColorSpace) return "sRGB";
	if (texture.colorSpace === NoColorSpace) return "Data";
	return "Linear";
}

function textureSize(
	texture: Texture,
): { width: number; height: number } | null {
	const img = texture.image as
		| { width?: number; height?: number }
		| null
		| undefined;
	return img?.width && img?.height
		? { width: img.width, height: img.height }
		: null;
}

/** The slot's bound texture: game slots through the extras, else three's maps. */
function boundTexture(material: Material, slot: string): Texture | null {
	const standard = material as unknown as Record<string, Texture | null>;
	const data = dataTexturesOf(material)[slot];
	if (data) return data;
	switch (slot) {
		case "_BaseMap":
			return standard.map ?? null;
		case "_BumpMap":
			return standard.normalMap ?? null;
		case "_EmissionMap":
			return standard.emissiveMap ?? null;
		default:
			return null;
	}
}

export function describeTextures(material: Material | null): TextureDetail[] {
	if (!material) return [];
	const extras = extrasOf(material);
	const out: TextureDetail[] = [];
	const seen = new Set<string>();
	const push = (
		slot: string,
		texture: Texture | null,
		uvSet: number | null,
	) => {
		const size = texture ? textureSize(texture) : null;
		out.push({
			slot,
			texture,
			name: texture?.name || null,
			width: size?.width ?? null,
			height: size?.height ?? null,
			colorSpace: texture ? colorSpaceLabel(texture) : null,
			uvSet,
		});
		seen.add(slot);
	};
	if (extras?.textures) {
		for (const slot of Object.keys(extras.textures)) {
			if (textureIndexOf(extras, slot) === null) continue;
			const entry = extras.textures[slot];
			const uvSet =
				entry && typeof entry === "object" && typeof entry.uvSet === "number"
					? entry.uvSet
					: null;
			push(slot, boundTexture(material, slot), uvSet);
		}
	}
	const standard = material as unknown as Record<string, Texture | null>;
	for (const [prop, label] of STANDARD_MAPS) {
		const texture = standard[prop];
		if (texture && !seen.has(label)) push(label, texture, null);
	}
	return out;
}

/** Everything the detail inspector shows for one mesh and material slot. */
export function describeMeshDetail(mesh: Mesh, materialIndex = 0): MeshDetail {
	const materials = Array.isArray(mesh.material)
		? mesh.material
		: [mesh.material];
	const index = Math.min(Math.max(materialIndex, 0), materials.length - 1);
	const material = originalMaterial(mesh, index);
	return {
		geometry: describeGeometry(mesh),
		material: describeMaterial(material),
		textures: describeTextures(material),
	};
}

/** Compact number for material floats: integers as-is, else up to 4 decimals. */
export function formatFloat(value: number): string {
	if (Number.isInteger(value)) return String(value);
	const abs = Math.abs(value);
	if (abs >= 1000 || (abs < 1e-4 && abs > 0)) return value.toExponential(2);
	return String(Number.parseFloat(value.toFixed(4)));
}
