/**
 * Inverted-hull outlines for NPR meshes: a BackSide SkinnedMesh sharing the
 * geometry and skeleton, extruded along normals averaged across coincident
 * vertices, with a width that is constant on screen (see OUTLINE_VERTEX).
 */
import {
	BackSide,
	type BufferAttribute,
	type BufferGeometry,
	Color,
	Float32BufferAttribute,
	type Material,
	type MeshStandardMaterial,
	ShaderMaterial,
	SkinnedMesh,
	type Texture,
} from "three";
import {
	dataTextureOf,
	extrasOf,
	flagOf,
	floatOf,
	type MaterialExtras,
} from "@/components/studio/npr/extras";
import { rigUniforms } from "@/components/studio/npr/rig";
import {
	OUTLINE_FRAGMENT,
	OUTLINE_VERTEX,
} from "@/components/studio/npr/shaders";
import { placeholderTexture } from "@/components/studio/npr/textures";

export const SMOOTH_NORMAL = "smoothNormal";

/** Adds averaged normals (positions matched at 1e-5) to a geometry once. */
export function ensureSmoothNormals(geometry: BufferGeometry): void {
	if (geometry.attributes[SMOOTH_NORMAL]) return;
	const pos = geometry.attributes.position as BufferAttribute | undefined;
	const nrm = geometry.attributes.normal as BufferAttribute | undefined;
	if (!pos || !nrm) return;
	const count = pos.count;
	const sums = new Map<string, [number, number, number]>();
	const keys = new Array<string>(count);
	for (let i = 0; i < count; i++) {
		const key = `${Math.round(pos.getX(i) * 1e5)},${Math.round(pos.getY(i) * 1e5)},${Math.round(pos.getZ(i) * 1e5)}`;
		keys[i] = key;
		const s = sums.get(key);
		if (s) {
			s[0] += nrm.getX(i);
			s[1] += nrm.getY(i);
			s[2] += nrm.getZ(i);
		} else {
			sums.set(key, [nrm.getX(i), nrm.getY(i), nrm.getZ(i)]);
		}
	}
	const out = new Float32Array(count * 3);
	for (let i = 0; i < count; i++) {
		const s = sums.get(keys[i]) as [number, number, number];
		const len = Math.hypot(s[0], s[1], s[2]);
		if (len > 1e-6) {
			out[i * 3] = s[0] / len;
			out[i * 3 + 1] = s[1] / len;
			out[i * 3 + 2] = s[2] / len;
		} else {
			out[i * 3] = nrm.getX(i);
			out[i * 3 + 1] = nrm.getY(i);
			out[i * 3 + 2] = nrm.getZ(i);
		}
	}
	geometry.setAttribute(SMOOTH_NORMAL, new Float32BufferAttribute(out, 3));
}

/** Whether a material asks for an outline hull. */
export function wantsOutline(
	source: Material,
	extras: MaterialExtras,
): boolean {
	return flagOf(extras, "_EnableOutline") && !source.transparent;
}

function outlineMaterial(source: Material, extras: MaterialExtras): Material {
	const s = source as MeshStandardMaterial;
	const map: Texture | null = s.map ?? null;
	const mask = flagOf(extras, "_EnableOutlineMask")
		? dataTextureOf(source, "_OutlineMask")
		: null;
	const defines: Record<string, string> = {};
	if (map) defines.HAS_MAP = "";
	if (s.alphaTest > 0) defines.ALPHA_TEST = "";
	if (mask) defines.OUTLINE_MASK = "";
	const material = new ShaderMaterial({
		name: `${source.name || "material"} (outline)`,
		vertexShader: OUTLINE_VERTEX,
		fragmentShader: OUTLINE_FRAGMENT,
		defines,
		uniforms: {
			uMap: { value: map ?? placeholderTexture() },
			uBaseColor: { value: s.color?.clone() ?? new Color(1, 1, 1) },
			uOpacity: { value: s.opacity ?? 1 },
			uAlphaTest: { value: s.alphaTest },
			uOutlineWidth: { value: floatOf(extras, "_OutlineWidth", 0.5) },
			uOutlineScale: rigUniforms.uOutlineScale,
			uOutlineOffsetZ: { value: floatOf(extras, "_OutlineOffsetZ", 0) },
			uOutlineMask: { value: mask ?? placeholderTexture() },
			uOutlineBrightness: {
				value: floatOf(extras, "_OutlineColorBrightness", 0.5),
			},
			uOutlineSaturation: {
				value: floatOf(extras, "_OutlineColorSaturation", 1),
			},
		},
		side: BackSide,
	});
	return material;
}

/**
 * Builds the hull for a skinned NPR mesh, or null when no material wants
 * one. Multi-material meshes get a hull material per group; groups whose
 * material opts out are drawn fully transparent via `visible: false`.
 */
export function createNprOutline(mesh: SkinnedMesh): SkinnedMesh | null {
	const sources = Array.isArray(mesh.material)
		? mesh.material
		: [mesh.material];
	const materials = sources.map((m) => {
		const extras = extrasOf(m);
		return extras && wantsOutline(m, extras)
			? outlineMaterial(m, extras)
			: null;
	});
	if (!materials.some(Boolean)) return null;
	ensureSmoothNormals(mesh.geometry);
	const resolved = materials.map((m) => {
		if (m) return m;
		const hidden = new ShaderMaterial({ visible: false });
		return hidden;
	});
	const hull = new SkinnedMesh(
		mesh.geometry,
		Array.isArray(mesh.material) ? resolved : resolved[0],
	);
	hull.bindMode = mesh.bindMode;
	hull.bind(mesh.skeleton, mesh.bindMatrix);
	hull.name = "outline";
	hull.frustumCulled = false;
	hull.raycast = () => {};
	hull.userData.studioOutline = true;
	hull.matrixAutoUpdate = false;
	hull.renderOrder = mesh.renderOrder;
	return hull;
}
