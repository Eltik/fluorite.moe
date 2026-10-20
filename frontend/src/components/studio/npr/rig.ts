/**
 * The character light rig: one key light that follows the camera plus the
 * per-character "volume" (tints, multipliers, range bias). Every NPR material
 * and outline shares these uniform objects, so updating them once per frame
 * drives the whole model.
 */
import { Color, type IUniform, MathUtils, Vector3 } from "three";
import type { StudioKind } from "@/lib/studio";

export type Rgb = [number, number, number];

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
