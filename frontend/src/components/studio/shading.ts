/**
 * Studio shading modes for loaded models.
 *
 * - Standard: the loader's PBR-ish MeshStandardMaterial (or the GLB's own).
 * - Toon: a MeshToonMaterial built from the same diffuse map with a stepped
 *   light ramp and a soft fresnel rim, plus an optional inverted-hull outline
 *   whose width is constant in screen pixels.
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
import type { ShadingId } from "@/components/studio/prefs";

/** Uniforms shared by every toon / outline material of the page. */
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

/** Meshes that never get an outline (eyes, brows, mouth interior…). */
const NO_OUTLINE =
	/(^|[_\s.-])(eye|eyes|eyeball|iris|pupil|brow|brows|eyebrow|lash|lashes|eyelash|mouth|teeth|tooth|tongue|hl|highlight|hs|shadow|eyeshadow|blush|cheek|fx|effect)([_\s.\d-]|$)/i;

interface ShadingData {
	standard: Material | Material[];
	toon?: Material | Material[];
	outline?: ThreeMesh | null;
}

type ShadedMesh = Mesh & { userData: { studioShading?: ShadingData } };

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

function makeOutline(mesh: Mesh): ThreeMesh | null {
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
	for (const mesh of meshesOf(root)) {
		let data = mesh.userData.studioShading;
		if (!data) {
			data = { standard: mesh.material };
			mesh.userData.studioShading = data;
		}
		if (mode === "toon") {
			data.toon ??= Array.isArray(data.standard)
				? data.standard.map(toonOf)
				: toonOf(data.standard);
			mesh.material = data.toon;
		} else {
			mesh.material = data.standard;
		}
		const wantOutline = mode === "toon" && outlines && !wireframe;
		if (wantOutline && data.outline === undefined) {
			data.outline = makeOutline(mesh);
			if (data.outline) mesh.add(data.outline);
		}
		if (data.outline) data.outline.visible = wantOutline;
		const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
		for (const m of mats) {
			if ("wireframe" in m) (m as { wireframe: boolean }).wireframe = wireframe;
		}
	}
}

/** Disposes the toon / outline variants created for a model. */
export function disposeShading(root: Object3D): void {
	for (const mesh of meshesOf(root)) {
		const data = mesh.userData.studioShading;
		if (!data) continue;
		const toon = data.toon;
		if (toon) {
			const originals = new Set(
				Array.isArray(data.standard) ? data.standard : [data.standard],
			);
			for (const m of Array.isArray(toon) ? toon : [toon]) {
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
		delete mesh.userData.studioShading;
	}
}
