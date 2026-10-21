/**
 * Mesh inspection for the View panel: what a picked mesh is (name, material,
 * game shader, triangles, texture slots) and which parts entry it belongs
 * to. Reads the original materials the loader produced, not the shading
 * variants, so the game's shader names survive toon / debug modes.
 */
import type { Material, Mesh, Object3D, Texture } from "three";
import type { LoadedModel, PartEntry } from "@/components/studio/load-model";
import {
	dataTexturesOf,
	extrasOf,
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
