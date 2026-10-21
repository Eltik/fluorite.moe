/**
 * Studio shading modes for loaded models.
 *
 * - Standard: character materials (`HGRP/CharacterNPR*` extras) go unlit
 *   through the game's tone curve, which is what the game's menu shows with
 *   toon shading off; anything else keeps the loader's MeshStandardMaterial.
 * - Toon: materials that carry `HGRP/CharacterNPR*` extras get the in-game
 *   NPR material (see `npr/`), eyes go unlit, and skinned meshes get an
 *   inverted-hull outline on averaged normals. Anything else (static parts,
 *   GLBs without extras) falls back to a MeshToonMaterial with a stepped ramp,
 *   a fresnel rim and a pixel-constant hull.
 *
 * Variants are created lazily, cached on each mesh's userData and disposed by
 * `disposeShading` (called from the model's dispose).
 */
import {
	BackSide,
	Color,
	DataTexture,
	type Material,
	type Mesh,
	MeshBasicMaterial,
	type MeshStandardMaterial,
	MeshToonMaterial,
	NearestFilter,
	type Object3D,
	RedFormat,
	SkinnedMesh,
	Mesh as ThreeMesh,
	Vector2,
	type WebGLProgramParametersWithUniforms,
} from "three";
import { extrasOf, isEyeShader, nprKind } from "@/components/studio/npr/extras";
import {
	computeFaceFrame,
	type FaceFrame,
	updateFaceUniforms,
} from "@/components/studio/npr/face-frame";
import {
	createEyeMaterial,
	createNprMaterial,
	createUnlitMaterial,
	type NprUserData,
	wantsFaceFrame,
} from "@/components/studio/npr/material";
import { createNprOutline } from "@/components/studio/npr/outline";
import type { ShadingId } from "@/components/studio/prefs";

/** Uniforms shared by the fallback toon / outline materials of the page. */
export const shadingUniforms = {
	rimColor: { value: new Color("#ffffff") },
	rimStrength: { value: 0.3 },
	outlinePx: { value: 1.5 },
	/** Drawing-buffer size in pixels. */
	resolution: { value: new Vector2(1, 1) },
	/** View depth at which the outline is exactly `outlinePx` wide. */
	outlineRef: { value: 4 },
};

/** Three-step light ramp: shadow, a narrow half-tone, lit. Linear values. */
function makeRamp(): DataTexture {
	const steps = [132, 132, 132, 132, 132, 196, 255, 255, 255, 255];
	const tex = new DataTexture(
		new Uint8Array(steps),
		steps.length,
		1,
		RedFormat,
	);
	tex.minFilter = NearestFilter;
	tex.magFilter = NearestFilter;
	tex.generateMipmaps = false;
	tex.needsUpdate = true;
	return tex;
}

let ramp: DataTexture | null = null;
function getRamp(): DataTexture {
	ramp ??= makeRamp();
	return ramp;
}

/** Fallback-path meshes that never get an outline (eyes, brows, mouth…). */
const NO_OUTLINE =
	/(^|[_\s.-])(eye|eyes|eyeball|iris|pupil|brow|brows|eyebrow|lash|lashes|eyelash|mouth|teeth|tooth|tongue|hl|highlight|hs|shadow|eyeshadow|blush|cheek|fx|effect)([_\s.\d-]|$)/i;

interface ShadingData {
	standard: Material | Material[];
	toon?: Material | Material[];
	/** Standard mode for character materials: unlit, tone mapped (as in the game's menu). */
	flat?: Material | Material[];
	outline?: ThreeMesh | null;
	/** Head axes for SDF-lit skin; `null` once tried and not applicable. */
	face?: FaceFrame | null;
	/** True when the toon variant uses the NPR materials. */
	npr?: boolean;
	renderOrder: number;
}

type ShadedMesh = Mesh & { userData: { studioShading?: ShadingData } };

// ------------------------------------------------------------- fallback toon

const toonCache = new WeakMap<Material, Material>();

function toonOf(source: Material): Material {
	const cached = toonCache.get(source);
	if (cached) return cached;
	const toon = createToon(source);
	toonCache.set(source, toon);
	return toon;
}

function createToon(source: Material): Material {
	const s = source as MeshStandardMaterial;
	if (!("map" in s) || !("color" in s)) return source;
	const toon = new MeshToonMaterial({
		name: `${s.name || "material"} (toon)`,
		color: s.color.clone(),
		map: s.map ?? null,
		gradientMap: getRamp(),
		side: s.side,
		transparent: s.transparent,
		opacity: s.opacity,
		alphaTest: s.alphaTest,
		alphaMap: s.alphaMap ?? null,
		vertexColors: s.vertexColors,
		emissive: s.emissive?.clone() ?? new Color(0),
		emissiveMap: s.emissiveMap ?? null,
		emissiveIntensity: s.emissiveIntensity ?? 1,
		depthWrite: s.depthWrite,
	});
	toon.onBeforeCompile = (shader: WebGLProgramParametersWithUniforms) => {
		shader.uniforms.rimColor = shadingUniforms.rimColor;
		shader.uniforms.rimStrength = shadingUniforms.rimStrength;
		shader.fragmentShader = `uniform vec3 rimColor;\nuniform float rimStrength;\n${shader.fragmentShader.replace(
			"#include <opaque_fragment>",
			`{
				float facing = abs(dot(normalize(normal), normalize(vViewPosition)));
				float rim = smoothstep(0.62, 0.86, 1.0 - facing);
				outgoingLight += diffuseColor.rgb * rimColor * rim * rimStrength;
			}
			#include <opaque_fragment>`,
		)}`;
	};
	toon.customProgramCacheKey = () => "studio-toon";
	return toon;
}

function outlineMaterial(source: Material): MeshBasicMaterial {
	const s = source as MeshStandardMaterial;
	const mat = new MeshBasicMaterial({
		name: "outline",
		color: new Color("#4a3c4f"),
		map: s.map ?? null,
		side: BackSide,
		alphaTest: s.alphaTest,
	});
	if (!s.map) mat.color.set("#2a2430");
	mat.onBeforeCompile = (shader: WebGLProgramParametersWithUniforms) => {
		shader.uniforms.outlinePx = shadingUniforms.outlinePx;
		shader.uniforms.resolution = shadingUniforms.resolution;
		shader.uniforms.outlineRef = shadingUniforms.outlineRef;
		shader.vertexShader = `uniform float outlinePx;\nuniform vec2 resolution;\nuniform float outlineRef;\n${shader.vertexShader.replace(
			"#include <project_vertex>",
			`#include <project_vertex>
			{
				vec3 viewNormal = normalize(normalMatrix * normal);
				vec2 clipDir = (projectionMatrix * vec4(viewNormal, 0.0)).xy;
				vec2 screenDir = clipDir * resolution;
				float len = length(screenDir);
				if (len > 1e-5) {
					float scale = clamp(outlineRef / max(gl_Position.w, 1e-4), 0.45, 1.6);
					vec2 ndc = (screenDir / len) * outlinePx * scale * 2.0 / resolution;
					gl_Position.xy += ndc * gl_Position.w;
					gl_Position.z += 1e-5 * gl_Position.w;
				}
			}`,
		)}`;
	};
	mat.customProgramCacheKey = () => "studio-outline";
	return mat;
}

function meshNames(mesh: Object3D): string {
	const names: string[] = [];
	for (let n: Object3D | null = mesh; n; n = n.parent) {
		if (n.name) names.push(n.name);
		if (names.length > 3) break;
	}
	return names.join(" ");
}

function makeFallbackOutline(mesh: Mesh): ThreeMesh | null {
	if (!mesh.geometry.attributes.normal) return null;
	if (NO_OUTLINE.test(meshNames(mesh))) return null;
	const sources = Array.isArray(mesh.material)
		? mesh.material
		: [mesh.material];
	if (sources.some((m) => m.transparent)) return null;
	const material = Array.isArray(mesh.material)
		? mesh.material.map(outlineMaterial)
		: outlineMaterial(mesh.material);
	let outline: ThreeMesh;
	if ((mesh as SkinnedMesh).isSkinnedMesh) {
		const skinned = mesh as SkinnedMesh;
		const o = new SkinnedMesh(mesh.geometry, material);
		o.bindMode = skinned.bindMode;
		o.bind(skinned.skeleton, skinned.bindMatrix);
		outline = o;
	} else {
		outline = new ThreeMesh(mesh.geometry, material);
	}
	outline.name = "outline";
	outline.frustumCulled = mesh.frustumCulled;
	outline.raycast = () => {};
	outline.userData.studioOutline = true;
	outline.matrixAutoUpdate = false;
	return outline;
}

// ------------------------------------------------------------- NPR

/** Whether any of a mesh's materials carries character shader extras. */
function hasNprExtras(materials: Material[]): boolean {
	return materials.some((m) => {
		const shader = extrasOf(m)?.shader;
		return nprKind(shader) !== null || isEyeShader(shader);
	});
}

function buildToonVariant(
	mesh: ShadedMesh,
	data: ShadingData,
): Material | Material[] {
	const build = (source: Material): Material => {
		const extras = extrasOf(source);
		const kind = nprKind(extras?.shader);
		if (extras && kind) {
			if (kind === "skin" && data.face === undefined) {
				data.face =
					wantsFaceFrame(extras) && (mesh as SkinnedMesh).isSkinnedMesh
						? computeFaceFrame(mesh as SkinnedMesh)
						: null;
			}
			return createNprMaterial(source, kind, extras, data.face ?? null);
		}
		if (extras && isEyeShader(extras.shader)) return createEyeMaterial(source);
		return toonOf(source);
	};
	const standard = data.standard;
	return Array.isArray(standard) ? standard.map(build) : build(standard);
}

/** Standard mode: NPR character materials go unlit; anything else keeps its PBR material. */
function buildFlatVariant(
	standard: Material | Material[],
): Material | Material[] {
	const build = (source: Material): Material => {
		const shader = extrasOf(source)?.shader;
		return nprKind(shader) !== null || isEyeShader(shader)
			? createUnlitMaterial(source, { exposure: true })
			: source;
	};
	return Array.isArray(standard) ? standard.map(build) : build(standard);
}

/**
 * The exporter currently writes vertex normals that sit ~40° off the mesh
 * geometry (the game's, like warfarin's, are within a few degrees of the
 * recomputed ones), which breaks every N·L-driven ramp. Until the exporter
 * is fixed, rebuild them from the triangles for lit character meshes.
 */
function repairNormals(mesh: Mesh, sources: Material[]): void {
	const g = mesh.geometry;
	if (g.userData.normalsRepaired) return;
	if (!sources.some((m) => nprKind(extrasOf(m)?.shader) !== null)) return;
	g.computeVertexNormals();
	g.userData.normalsRepaired = true;
}

/** Render queue from the extras of the mesh's (first) material, or -1. */
function renderQueueOf(materials: Material[]): number {
	for (const m of materials) {
		const q = extrasOf(m)?.renderQueue;
		if (typeof q === "number" && q >= 0) return q;
	}
	return -1;
}

function meshesOf(root: Object3D): ShadedMesh[] {
	const meshes: ShadedMesh[] = [];
	root.traverse((node) => {
		const mesh = node as ShadedMesh;
		if (mesh.isMesh && !mesh.userData.studioOutline) meshes.push(mesh);
	});
	return meshes;
}

/** Applies a shading mode, outline toggle and wireframe to a model. */
export function applyShading(
	root: Object3D,
	mode: ShadingId,
	outlines: boolean,
	wireframe: boolean,
): void {
	if (import.meta.env.DEV) {
		// Dev-only: lets the console inspect the shaded model.
		(window as unknown as { __studioModel?: Object3D }).__studioModel = root;
	}
	for (const mesh of meshesOf(root)) {
		let data = mesh.userData.studioShading;
		if (!data) {
			data = { standard: mesh.material, renderOrder: mesh.renderOrder };
			mesh.userData.studioShading = data;
		}
		const sources = Array.isArray(data.standard)
			? data.standard
			: [data.standard];
		repairNormals(mesh, sources);
		const queue = renderQueueOf(sources);
		if (mode === "toon") {
			if (!data.toon) {
				data.npr = hasNprExtras(sources);
				data.toon = buildToonVariant(mesh, data);
			}
			mesh.material = data.toon;
			mesh.renderOrder = queue >= 0 ? queue : data.renderOrder;
		} else {
			data.flat ??= buildFlatVariant(data.standard);
			mesh.material = data.flat;
			mesh.renderOrder = queue >= 0 ? queue : data.renderOrder;
		}
		const wantOutline = mode === "toon" && outlines && !wireframe;
		if (wantOutline && data.outline === undefined) {
			data.outline = data.npr
				? (mesh as SkinnedMesh).isSkinnedMesh
					? createNprOutline(mesh as SkinnedMesh)
					: null
				: makeFallbackOutline(mesh);
			if (data.outline) mesh.add(data.outline);
		}
		if (data.outline) {
			data.outline.visible = wantOutline;
			data.outline.renderOrder = mesh.renderOrder;
		}
		const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
		for (const m of mats) {
			if ("wireframe" in m) (m as { wireframe: boolean }).wireframe = wireframe;
		}
	}
}

/** Pushes the current head axes into every SDF-lit skin material. */
export function updateFaceFrames(root: Object3D): void {
	for (const mesh of meshesOf(root)) {
		const data = mesh.userData.studioShading;
		if (!data?.face || !data.toon) continue;
		const mats = Array.isArray(data.toon) ? data.toon : [data.toon];
		for (const m of mats) {
			const face = (m.userData as Partial<NprUserData>).face;
			if (face) updateFaceUniforms(data.face, face);
		}
	}
}

/** Disposes the toon / outline variants created for a model. */
export function disposeShading(root: Object3D): void {
	for (const mesh of meshesOf(root)) {
		const data = mesh.userData.studioShading;
		if (!data) continue;
		const originals = new Set(
			Array.isArray(data.standard) ? data.standard : [data.standard],
		);
		for (const variant of [data.toon, data.flat]) {
			if (!variant) continue;
			for (const m of Array.isArray(variant) ? variant : [variant]) {
				if (!originals.has(m)) m.dispose();
			}
		}
		const outline = data.outline;
		if (outline) {
			const mats = Array.isArray(outline.material)
				? outline.material
				: [outline.material];
			for (const m of mats) m.dispose();
			outline.removeFromParent();
		}
		// Restore the original so the caller disposes what it created.
		mesh.material = data.standard;
		mesh.renderOrder = data.renderOrder;
		delete mesh.userData.studioShading;
	}
}
