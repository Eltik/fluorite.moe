/**
 * Builds the in-game look for `HGRP/CharacterNPR*` materials: one
 * ShaderMaterial whose defines and uniforms come from the glTF material and
 * its `extras`. Eyes (`_Eye`) are unlit. Data textures the exporter has not
 * embedded degrade to their documented fallbacks.
 */
import {
	Color,
	CustomBlending,
	type Material,
	MeshBasicMaterial,
	type MeshStandardMaterial,
	OneFactor,
	OneMinusSrcAlphaFactor,
	ShaderMaterial,
	type Texture,
	Vector2,
	Vector4,
} from "three";
import {
	colorOf,
	dataTextureOf,
	flagOf,
	floatOf,
	hasSlot,
	type MaterialExtras,
	type NprKind,
	vectorOf,
} from "@/components/studio/npr/extras";
import {
	createFaceUniforms,
	type FaceFrame,
	type FaceUniforms,
} from "@/components/studio/npr/face-frame";
import {
	addLightUniforms,
	rigUniforms,
	shadowUniforms,
} from "@/components/studio/npr/rig";
import {
	NPR_FRAGMENT,
	NPR_VERTEX,
	TONE_CURVE_GLSL,
} from "@/components/studio/npr/shaders";
import {
	fallbackHairSpecRamp,
	fallbackRamp,
	placeholderTexture,
} from "@/components/studio/npr/textures";

/** Any of these non-zero keeps cloth off the stylized PBR path. */
const PBR_EXCLUDES = [
	"_SilkStockings",
	"_Pantyhose",
	"_ClearCoat",
	"_UseAnisotropy",
	"_UseSpecRampMap",
	"_EnableRealisticLighting",
	"_UseCharacterErosion",
	"_UsePuppet",
	"_UsePuppetProceduralDCurve",
	"_UseCharacterFur",
	"_EnableStylizedFresnel",
	"_UseUV2Color",
];

export interface NprUserData {
	npr: true;
	/** Present on skin materials that drive the SDF face shadow. */
	face?: FaceUniforms;
}

/** Whether a skin material wants the head axes computed for its mesh. */
export function wantsFaceFrame(extras: MaterialExtras): boolean {
	return flagOf(extras, "_UseSDFLightmap") && hasSlot(extras, "_SDFLightmap");
}

/** A 2D-unwrapped 3D LUT is an N×N² strip. */
function lutSize(lut: Texture): number | null {
	const img = lut.image as { width?: number; height?: number } | undefined;
	if (!img?.width || !img.height) return null;
	return img.width === img.height * img.height ? img.height : null;
}

export function createNprMaterial(
	source: Material,
	kind: NprKind,
	extras: MaterialExtras,
	face: FaceFrame | null,
): ShaderMaterial {
	const s = source as MeshStandardMaterial;
	const f = (name: string, fallback: number) => floatOf(extras, name, fallback);
	const on = (name: string) => flagOf(extras, name);
	const tex = (slot: string) => dataTextureOf(source, slot);
	const map: Texture | null = s.map ?? null;

	const diffRamp = on("_UseDiffRampMap")
		? (tex("_DiffRampMap") ??
			(hasSlot(extras, "_DiffRampMap") ? fallbackRamp(kind) : null))
		: null;
	const specRamp = on("_UseSpecRampMap")
		? (tex("_SpecRampMap") ??
			(kind === "hair" && hasSlot(extras, "_SpecRampMap")
				? fallbackHairSpecRamp()
				: null))
		: null;
	const pMap = kind !== "skin" ? tex("_MetallicGlossMap") : null;
	// A declared but missing P map falls back to the material constants.
	const pConst =
		kind !== "skin" &&
		!pMap &&
		on("_UseMetallicGlossMap") &&
		hasSlot(extras, "_MetallicGlossMap");
	const useMetalGloss = on("_UseMetallicGlossMap") && (pMap !== null || pConst);
	const sdf =
		kind === "skin" && face && on("_UseSDFLightmap")
			? tex("_SDFLightmap")
			: null;
	const faceFlat = kind === "skin" && face && on("_UseSDFLightmap") && !sdf;
	const sdfMask = sdf ? tex("_SDFMask") : null;
	const highlight = sdf ? tex("_HighlightMap") : null;
	const lut = on("_UseShadowLutTex") ? tex("_ShadowLutTex") : null;
	const lutN = lut ? lutSize(lut) : null;
	const pbrCloth =
		kind === "cloth" &&
		!diffRamp &&
		PBR_EXCLUDES.every((name) => f(name, 0) === 0);
	const emissionMap =
		kind === "cloth" && on("_UseEmission")
			? (tex("_EmissionMap") ?? s.emissiveMap ?? null)
			: null;

	// Unity alpha-blended surfaces with premultiplied output.
	const premulBlend =
		f("_SurfaceType", 0) === 1 &&
		f("_SrcBlend", 1) === 1 &&
		f("_DstBlend", 0) === 10;
	const alphaPremul = f("_AlphaPremultiply", 0);
	const transparent = s.transparent || premulBlend;
	// The exporter marks some opaque materials MASK; trust the shader flags.
	const alphaTest =
		extras.floats?._EnableAlphaTest === 0 && f("_AlphaClip", 0) === 0
			? 0
			: s.alphaTest;

	const defines: Record<string, string> = {};
	defines[`NPR_${kind.toUpperCase()}`] = "";
	if (map) defines.HAS_MAP = "";
	if (alphaTest > 0) defines.ALPHA_TEST = "";
	if (transparent) defines.TRANSPARENT = "";
	if (diffRamp) defines.USE_DIFF_RAMP = "";
	if (specRamp) defines.USE_SPEC_RAMP = "";
	if (pMap) defines.HAS_P_MAP = "";
	if (pConst) defines.P_CONST = "";
	if (useMetalGloss) defines.USE_METAL_GLOSS = "";
	if (sdf) defines.FACE_SDF = "";
	if (faceFlat) defines.FACE_FLAT = "";
	if (sdfMask) defines.SDF_MASK = "";
	if (highlight) defines.HIGHLIGHT = "";
	if (lut && lutN) defines.SHADOW_LUT = "";
	if (pbrCloth) defines.PBR_CLOTH = "";
	if (emissionMap) defines.EMISSION = "";
	if (premulBlend && alphaPremul === 1) defines.PREMUL = "";

	const baseColor = colorOf(extras, "_BaseColor");
	const emission = colorOf(extras, "_EmissionColor");
	const highlightVec = vectorOf(extras, "_HighlightMapVector");
	const faceUniforms = face ? createFaceUniforms() : null;
	const fill = placeholderTexture();

	const material = new ShaderMaterial({
		name: `${source.name || "material"} (npr)`,
		vertexShader: NPR_VERTEX,
		fragmentShader: NPR_FRAGMENT,
		defines,
		uniforms: {
			uMap: { value: map ?? fill },
			uBaseColor: {
				value: baseColor?.color ?? s.color?.clone() ?? new Color(1, 1, 1),
			},
			uOpacity: { value: (s.opacity ?? 1) * (baseColor?.alpha ?? 1) },
			uAlphaTest: { value: alphaTest },
			uDiffRamp: { value: diffRamp ?? fill },
			uSpecRamp: { value: specRamp ?? fill },
			uMetalGloss: { value: pMap ?? fill },
			uPbrConst: {
				value:
					on("_UseMetallicGlossMap") && pMap
						? new Vector4(1, 1, 1, 1)
						: new Vector4(
								f("_Metallic", 0),
								f("_Specular", 1),
								1,
								f("_Smoothness", 0.5),
							),
			},
			uSdf: { value: sdf ?? fill },
			uSdfMask: { value: sdfMask ?? fill },
			uShadowLut: { value: lut ?? fill },
			uLutSize: { value: lutN ?? 1 },
			uShadowBrightness: { value: f("_ShadowColorBrightness", 0.5) },
			uShadowSaturation: { value: f("_ShadowColorSaturation", 1) },
			uSpecular: { value: f("_Specular", 1) },
			uRimEnable: { value: kind === "skin" ? 0 : 1 },
			uSdfRimColor: {
				value: colorOf(extras, "_SDFRimColor")?.color ?? new Color(1, 1, 1),
			},
			uSkinRimOffScale: { value: f("_SkinRimOffScale", 1) },
			uFaceRimOffScale: { value: f("_FaceRimOffScale", 1) },
			uHighlightMap: { value: highlight ?? fill },
			uHighlightVec: {
				value: highlightVec
					? new Vector2(highlightVec[0], highlightVec[1])
					: new Vector2(),
			},
			...(faceUniforms ?? createFaceUniforms()),
			uAlphaPremul: { value: alphaPremul },
			uEmissionMap: { value: emissionMap ?? fill },
			uEmissionColor: {
				value: (emission?.color ?? new Color(0, 0, 0)).multiplyScalar(
					f("_EmissionBrightness", 1),
				),
			},
			uLightDir: rigUniforms.uLightDir,
			uLightColor: rigUniforms.uLightColor,
			uSkinLightColor: rigUniforms.uSkinLightColor,
			uShadowTint:
				kind === "skin" ? rigUniforms.uSkinShadowTint : rigUniforms.uShadowTint,
			uRangeBias: rigUniforms.uRangeBias,
			uEnvMul: rigUniforms.uEnvMul,
			uEnvShadowMul: rigUniforms.uEnvShadowMul,
			uRimColor: rigUniforms.uRimColor,
			uRimWidth: rigUniforms.uRimWidth,
			uSpecStrength: rigUniforms.uSpecStrength,
			...addLightUniforms,
			...shadowUniforms,
		},
		side: s.side,
		transparent,
		depthTest: s.depthTest,
		depthWrite: premulBlend ? f("_ZWrite", 1) !== 0 : s.depthWrite,
	});
	if (premulBlend) {
		material.blending = CustomBlending;
		material.blendSrc = OneFactor;
		material.blendDst = OneMinusSrcAlphaFactor;
		material.premultipliedAlpha = true;
	} else {
		material.blending = s.blending;
	}
	// An alpha-blended, premultiplied, fully transparent tint with no map
	// draws nothing in-game.
	if (
		f("_SurfaceType", 0) === 1 &&
		alphaPremul === 1 &&
		(baseColor?.alpha ?? 1) === 0 &&
		!map
	) {
		material.visible = false;
	}
	const data: NprUserData = { npr: true };
	if (faceUniforms) data.face = faceUniforms;
	material.userData = data;
	return material;
}

/**
 * Unlit albedo through the game's tone curve. This is both the iris / brow
 * look in toon mode and, with `exposure`, the whole character in the
 * Standard mode (the game's menu renders no lighting there either, only the
 * tone curve over the base colour).
 */
export function createUnlitMaterial(
	source: Material,
	options: { exposure?: boolean } = {},
): MeshBasicMaterial {
	const s = source as MeshStandardMaterial;
	const exposure = options.exposure === true;
	const material = new MeshBasicMaterial({
		name: `${source.name || "material"} (${exposure ? "flat" : "eye"})`,
		map: s.map ?? null,
		color: s.color?.clone() ?? new Color(1, 1, 1),
		alphaTest: s.alphaTest,
		transparent: s.transparent,
		opacity: s.opacity,
		side: s.side,
		depthWrite: s.depthWrite,
		// The renderer's tone mapping is bypassed; the curve below is applied
		// instead. With `toneMapped` off three omits its exposure declaration,
		// so it is declared here; the renderer still uploads the value.
		toneMapped: false,
	});
	const input = exposure
		? "gl_FragColor.rgb * toneMappingExposure"
		: "gl_FragColor.rgb";
	const declare = exposure ? "uniform float toneMappingExposure;\n" : "";
	material.onBeforeCompile = (shader) => {
		shader.fragmentShader = shader.fragmentShader
			.replace(
				"#include <common>",
				`#include <common>\n${declare}${TONE_CURVE_GLSL}`,
			)
			.replace(
				"#include <tonemapping_fragment>",
				`gl_FragColor.rgb = nprToneMap(${input});`,
			);
	};
	material.customProgramCacheKey = () => (exposure ? "npr-flat" : "npr-eye");
	return material;
}

/** Iris and brows: unlit, through the same tone curve. */
export function createEyeMaterial(source: Material): MeshBasicMaterial {
	return createUnlitMaterial(source);
}

export function isNprMaterial(material: Material): material is ShaderMaterial {
	return (material.userData as Partial<NprUserData>).npr === true;
}
