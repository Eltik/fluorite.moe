/**
 * The character light rig: one key light that follows the camera plus the
 * per-character "volume" (tints, multipliers, range bias). Every NPR material
 * and outline shares these uniform objects, so updating them once per frame
 * drives the whole model.
 */
import {
	Color,
	DepthTexture,
	type IUniform,
	LessEqualCompare,
	LinearFilter,
	MathUtils,
	Matrix4,
	UnsignedShortType,
	Vector2,
	Vector3,
	Vector4,
	WebGLRenderTarget,
} from "three";
import type { StudioKind } from "@/lib/studio";

export type Rgb = [number, number, number];
export type Vec3 = [number, number, number];
export type Vec4 = [number, number, number, number];

/** A light from the character's menu light group (positions in three.js handedness). */
export interface CharacterLight {
	name?: string;
	active: boolean;
	type: "point" | "spot" | "directional" | "area";
	/** Linear colour. */
	colorLinear: Rgb;
	intensity: number;
	/** < 0 selects the inverse-square falloff. */
	falloffExponent?: number;
	range: number;
	/** Spot angles in degrees (full): outer, inner. */
	spot?: [number, number];
	/** Model-space position and forward, used when there is no follower. */
	position: Vec3;
	forward: Vec3;
	npr: "wrap" | "rim" | "ramped" | "specular" | "fog" | "unknown";
	/** wrap: amount, auto-limit; rim: width, albedo mix. */
	nprData: Vec4;
	follower?: {
		type: "parent" | "fixedOffset";
		/** Bone name (`Bip001`, `Head_Local`). */
		node: string;
		positionOffset?: Vec3;
		localPosition?: Vec3;
		localRotation?: Vec4;
	} | null;
}

/** The whole menu lighting entry for a character. */
export interface CharacterLighting {
	volume: CharacterVolume;
	lights: CharacterLight[];
}

export const MAX_ADD_LIGHTS = 16;

export interface AddLightUniforms {
	uAddLightCount: IUniform<number>;
	uAddLightPos: IUniform<Vector4[]>;
	uAddLightColor: IUniform<Vector4[]>;
	uAddLightDir: IUniform<Vector4[]>;
	uAddLightCone: IUniform<Vector4[]>;
	uAddLightData: IUniform<Vector4[]>;
}

const vec4s = () => Array.from({ length: MAX_ADD_LIGHTS }, () => new Vector4());

/** Packed additional lights, shared by every NPR material. */
export const addLightUniforms: AddLightUniforms = {
	uAddLightCount: { value: 0 },
	uAddLightPos: { value: vec4s() },
	uAddLightColor: { value: vec4s() },
	uAddLightDir: { value: vec4s() },
	uAddLightCone: { value: vec4s() },
	uAddLightData: { value: vec4s() },
};

/**
 * Per-character menu lighting (the game's `HGCharacterVolume`). Colours are
 * linear and used as given. Tints are normalised around 1.
 */
export interface CharacterVolume {
	/** Key light intensity multiplier. */
	multiplier: number;
	/** Scales the shadow colour (ambient/env term). */
	envMul: number;
	/** Scales the fake environment reflection on PBR cloth. */
	envShadowMul: number;
	/** Added to N·L before the ramp; negative pushes the terminator towards the light. */
	rangeBias: number;
	/** Lowest key light elevation in degrees. */
	minPitch: number;
	/** Key light yaw offset from the camera in degrees. */
	yawBias: number;
	lightColor: Rgb;
	skinLightColor: Rgb;
	shadowTint: Rgb;
	skinShadowTint: Rgb;
}

/** The global `charinfo_volume/HGCharacterVolume` defaults (operators). */
export const DEFAULT_VOLUME: CharacterVolume = {
	multiplier: 0.9,
	envMul: 0.8,
	envShadowMul: 0.8,
	rangeBias: -0.4,
	minPitch: 32,
	yawBias: 10,
	lightColor: [1, 1, 1],
	skinLightColor: [1, 1, 1],
	shadowTint: [0.878, 0.93, 1.122],
	skinShadowTint: [1.187, 0.927, 0.813],
};

/** Defaults for NPCs and enemies, which have no menu volume. */
export const NPC_VOLUME: CharacterVolume = {
	multiplier: 1.05,
	envMul: 0.55,
	envShadowMul: 0.85,
	rangeBias: 0,
	minPitch: 32,
	yawBias: 10,
	lightColor: [1, 1, 1],
	skinLightColor: [1, 1, 1],
	shadowTint: [0.8783, 0.9302, 1.1217],
	skinShadowTint: [1.187, 0.9272, 0.813],
};

/**
 * The volume for a catalog entry. Per-character volumes are not exported yet;
 * when they are, this is where `/api/...` plugs in.
 */
export function volumeFor(kind: StudioKind): CharacterVolume {
	return kind === "operators" ? DEFAULT_VOLUME : NPC_VOLUME;
}

/** Rig constants that are not part of the volume. */
export const RIG = {
	rimColor: 0.25,
	rimWidth: 0.3,
	specStrength: 0.5,
	outlineScale: 0.006,
} as const;

export interface RigUniforms {
	uLightDir: IUniform<Vector3>;
	uLightColor: IUniform<Color>;
	uSkinLightColor: IUniform<Color>;
	uShadowTint: IUniform<Color>;
	uSkinShadowTint: IUniform<Color>;
	uRangeBias: IUniform<number>;
	uEnvMul: IUniform<number>;
	uEnvShadowMul: IUniform<number>;
	uRimColor: IUniform<Color>;
	uRimWidth: IUniform<number>;
	uSpecStrength: IUniform<number>;
	uOutlineScale: IUniform<number>;
}

/** Shared by every NPR material of the page; mutate `.value` only. */
export const rigUniforms: RigUniforms = {
	uLightDir: { value: new Vector3(0, 0.53, 0.85) },
	uLightColor: { value: new Color(1, 1, 1) },
	uSkinLightColor: { value: new Color(1, 1, 1) },
	uShadowTint: { value: new Color(1, 1, 1) },
	uSkinShadowTint: { value: new Color(1, 1, 1) },
	uRangeBias: { value: DEFAULT_VOLUME.rangeBias },
	uEnvMul: { value: DEFAULT_VOLUME.envMul },
	uEnvShadowMul: { value: DEFAULT_VOLUME.envShadowMul },
	uRimColor: { value: new Color(RIG.rimColor, RIG.rimColor, RIG.rimColor) },
	uRimWidth: { value: RIG.rimWidth },
	uSpecStrength: { value: RIG.specStrength },
	uOutlineScale: { value: RIG.outlineScale },
};

export const SHADOW_MAP_SIZE = 1024;

export interface ShadowUniforms {
	uShadowMap: IUniform<DepthTexture>;
	uShadowMatrix: IUniform<Matrix4>;
	/** World-space offsets along the light and the normal (1 and 4 texels). */
	uShadowBias: IUniform<Vector2>;
	uShadowTexel: IUniform<number>;
	uShadowOn: IUniform<number>;
}

let shadowTarget: WebGLRenderTarget | null = null;

/**
 * The self-shadow depth map. The texture compares on sampling (a
 * `sampler2DShadow` with linear filtering), so every tap is already a
 * bilinear blend of four depth tests.
 */
export function getShadowTarget(): WebGLRenderTarget {
	if (shadowTarget) return shadowTarget;
	const depth = new DepthTexture(
		SHADOW_MAP_SIZE,
		SHADOW_MAP_SIZE,
		UnsignedShortType,
	);
	depth.minFilter = LinearFilter;
	depth.magFilter = LinearFilter;
	depth.compareFunction = LessEqualCompare;
	shadowTarget = new WebGLRenderTarget(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE, {
		depthTexture: depth,
		depthBuffer: true,
		stencilBuffer: false,
	});
	return shadowTarget;
}

/** Shared by every NPR material; filled by `SelfShadow` each frame. */
export const shadowUniforms: ShadowUniforms = {
	uShadowMap: { value: getShadowTarget().depthTexture as DepthTexture },
	uShadowMatrix: { value: new Matrix4() },
	uShadowBias: { value: new Vector2(0, 0) },
	uShadowTexel: { value: 1 / SHADOW_MAP_SIZE },
	uShadowOn: { value: 0 },
};

export function applyVolume(volume: CharacterVolume): void {
	const u = rigUniforms;
	const m = volume.multiplier;
	u.uLightColor.value.setRGB(...volume.lightColor).multiplyScalar(m);
	u.uSkinLightColor.value.setRGB(...volume.skinLightColor).multiplyScalar(m);
	u.uShadowTint.value.setRGB(...volume.shadowTint);
	u.uSkinShadowTint.value.setRGB(...volume.skinShadowTint);
	u.uRangeBias.value = volume.rangeBias;
	u.uEnvMul.value = volume.envMul;
	u.uEnvShadowMul.value = volume.envShadowMul;
}

/**
 * Unit vector towards the key light for a camera forward direction: the
 * camera's yaw plus `yawBias`, and its pitch clamped to at least `minPitch`.
 */
export function keyLightDirection(
	forward: Vector3,
	volume: CharacterVolume,
	out: Vector3,
): Vector3 {
	const yaw =
		Math.atan2(-forward.x, forward.z) + MathUtils.degToRad(volume.yawBias);
	const pitch = Math.max(
		MathUtils.degToRad(volume.minPitch),
		Math.asin(MathUtils.clamp(-forward.y, -1, 1)),
	);
	const cp = Math.cos(pitch);
	return out.set(Math.sin(yaw) * cp, Math.sin(pitch), -Math.cos(yaw) * cp);
}

/** The forward direction of the default front view (light follow off). */
export const FRONT_VIEW_FORWARD = new Vector3(0, 0, -1);
