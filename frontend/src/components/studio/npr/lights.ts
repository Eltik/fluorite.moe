/**
 * Places a character's additional wrap / rim lights every frame and packs
 * them into the shared uniform arrays. Lights follow a bone (`parent`), sit
 * at a fixed offset from one in model orientation (`fixedOffset`), or are
 * static in model space.
 */
import { MathUtils, type Object3D, Quaternion, Vector3 } from "three";
import {
	addLightUniforms,
	type CharacterLight,
	MAX_ADD_LIGHTS,
} from "@/components/studio/npr/rig";

interface Placed {
	light: CharacterLight;
	node: Object3D | null;
	localPosition: Vector3;
	localRotation: Quaternion;
	positionOffset: Vector3;
	forward: Vector3;
	position: Vector3;
}

const worldPos = new Vector3();
const worldQuat = new Quaternion();
const modelQuat = new Quaternion();
const tmp = new Vector3();
const dir = new Vector3();

/** Lights the shader uses: active wrap / rim point and spot lights. */
export function usableLights(lights: CharacterLight[]): CharacterLight[] {
	return lights
		.filter(
			(l) =>
				l.active &&
				(l.npr === "wrap" || l.npr === "rim") &&
				l.type !== "directional" &&
				l.type !== "area",
		)
		.sort((a, b) => b.intensity - a.intensity)
		.slice(0, MAX_ADD_LIGHTS);
}

function findNode(root: Object3D, name: string): Object3D | null {
	let found: Object3D | null = null;
	root.traverse((n) => {
		if (!found && n.name === name) found = n;
	});
	return found;
}

export class AdditionalLights {
	private readonly placed: Placed[];

	constructor(
		private readonly root: Object3D,
		lights: CharacterLight[],
	) {
		this.placed = usableLights(lights).map((light) => {
			const f = light.follower;
			return {
				light,
				node: f ? findNode(root, f.node) : null,
				localPosition: new Vector3(...(f?.localPosition ?? [0, 0, 0])),
				localRotation: new Quaternion(...(f?.localRotation ?? [0, 0, 0, 1])),
				positionOffset: new Vector3(...(f?.positionOffset ?? [0, 0, 0])),
				forward: new Vector3(...light.forward),
				position: new Vector3(...light.position),
			};
		});
	}

	get count(): number {
		return this.placed.length;
	}

	/** Packs the lights for the current pose; `stageFactor` dims them (0.5 on the dark stage). */
	update(stageFactor: number): void {
		const u = addLightUniforms;
		this.root.getWorldQuaternion(modelQuat);
		this.placed.forEach((p, i) => {
			const { light, node } = p;
			const follower = light.follower;
			if (follower && node) {
				node.getWorldPosition(worldPos);
				if (follower.type === "parent") {
					node.getWorldQuaternion(worldQuat);
					tmp.copy(p.localPosition).applyQuaternion(worldQuat).add(worldPos);
					dir.set(0, 0, 1).applyQuaternion(worldQuat.multiply(p.localRotation));
				} else {
					tmp.copy(p.positionOffset).applyQuaternion(modelQuat).add(worldPos);
					dir.copy(p.forward).applyQuaternion(modelQuat);
				}
			} else {
				tmp.copy(p.position).applyMatrix4(this.root.matrixWorld);
				dir.copy(p.forward).applyQuaternion(modelQuat);
			}
			u.uAddLightPos.value[i].set(
				tmp.x,
				tmp.y,
				tmp.z,
				1 / Math.max(light.range, 1e-4),
			);
			const [r, g, b] = light.colorLinear;
			const k = light.intensity * stageFactor;
			u.uAddLightColor.value[i].set(
				r * k,
				g * k,
				b * k,
				light.npr === "rim" ? 3 : 0,
			);
			u.uAddLightDir.value[i].set(
				dir.x,
				dir.y,
				dir.z,
				light.falloffExponent ?? -1,
			);
			if (light.type === "spot" && light.spot) {
				const outer = Math.cos(MathUtils.degToRad(light.spot[0]) / 2);
				const inner = Math.cos(MathUtils.degToRad(light.spot[1]) / 2);
				u.uAddLightCone.value[i].set(
					outer,
					1 / Math.max(inner - outer, 1e-4),
					1,
					0,
				);
			} else {
				u.uAddLightCone.value[i].set(-1, 0, 0, 0);
			}
			u.uAddLightData.value[i].set(...light.nprData);
		});
		u.uAddLightCount.value = this.placed.length;
	}
}

/** Turns the additional lights off (restores the built-in rim). */
export function clearAdditionalLights(): void {
	addLightUniforms.uAddLightCount.value = 0;
}
