/**
 * Loads the default weapon and the deco props of a model, hangs them on their
 * mount nodes, shades them like the character and drives their visibility
 * from the clip time (see `lib/props` for the rules).
 */
import type { Group, Material, Mesh, Object3D } from "three";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { fetchBytes } from "@/components/studio/bytes";
import {
	disposeObject,
	type LoadedModel,
	resolveDataTextures,
} from "@/components/studio/load-model";
import { extrasOf, isVfxShader } from "@/components/studio/npr/extras";
import {
	applyShading,
	disposeShading,
	type ShadingLook,
} from "@/components/studio/shading";
import { modelFileUrl } from "@/lib/api";
import type { ClipSource } from "@/lib/clips";
import {
	type DecoItem,
	decoVisibility,
	nodeName,
	type WeaponInfo,
	weaponVisible,
} from "@/lib/props";

// ------------------------------------------------------------- GLBs

let loader: GLTFLoader | null = null;
function getLoader(): GLTFLoader {
	if (!loader) {
		loader = new GLTFLoader();
		loader.setMeshoptDecoder(MeshoptDecoder);
	}
	return loader;
}

const LOD_NAME = /_lod([1-9]\d*)(?:$|[^0-9])/i;

/** Hides game-side helpers that never draw: LOD levels and VFX meshes. */
function hideHelpers(root: Object3D): void {
	root.traverse((node) => {
		const mesh = node as Mesh;
		if (!mesh.isMesh) return;
		mesh.frustumCulled = false;
		if (LOD_NAME.test(mesh.name)) {
			mesh.visible = false;
			return;
		}
		const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
		if (mats.some((m: Material) => isVfxShader(extrasOf(m)?.shader))) {
			mesh.visible = false;
		}
	});
}

async function loadPropGlb(url: string, signal: AbortSignal): Promise<Group> {
	const buf = await fetchBytes(url, { signal, onProgress: () => {} });
	const gltf = await getLoader().parseAsync(buf, "");
	await resolveDataTextures(gltf);
	if (signal.aborted) {
		disposeObject(gltf.scene);
		throw new DOMException("Aborted", "AbortError");
	}
	hideHelpers(gltf.scene);
	return gltf.scene;
}

// ------------------------------------------------------------- rig

interface MountedWeapon {
	root: Object3D;
}

interface MountedDeco {
	item: DecoItem;
	root: Object3D;
	/** Objects to toggle per visibility node path. */
	targets: Map<string, Object3D[]>;
	/** The helper visibility (LODs, VFX) each object had after loading. */
	baseline: Map<Object3D, boolean>;
}

export type Look = ShadingLook;

/**
 * The default weapon and deco props of one model: loaded once, attached to
 * their mount nodes, shaded like the character and shown or hidden per clip
 * time. Dispose before the model goes away.
 */
export class PropsRig {
	readonly model: LoadedModel;
	readonly warnings: string[] = [];
	private weapons: MountedWeapon[] = [];
	private decos: MountedDeco[] = [];
	private weaponOn = true;
	private propsOn = true;
	private lastClip: string | null | undefined;
	private lastTime = Number.NaN;
	private lastWeaponVisible: boolean | null = null;
	private disposed = false;
	/** Every root the rig added to the model, for shading and disposal. */
	private roots: Object3D[] = [];

	constructor(model: LoadedModel) {
		this.model = model;
	}

	get hasWeapon(): boolean {
		return this.weapons.length > 0;
	}

	get hasProps(): boolean {
		return this.decos.length > 0;
	}

	/** Loads and mounts everything. Resolves even when some files fail. */
	async load(signal: AbortSignal, look: Look): Promise<void> {
		const cs = this.model.charscreen;
		if (!cs) return;
		const jobs: Promise<void>[] = [];
		if (cs.weapon?.glb && cs.weapon.mounts.length) {
			const info = cs.weapon;
			jobs.push(
				loadPropGlb(modelFileUrl(info.glb as string), signal).then(
					(scene) => {
						if (signal.aborted || this.disposed) {
							disposeObject(scene);
							return;
						}
						this.mountWeapon(scene, info, look);
					},
					(error: unknown) => {
						if (signal.aborted) throw error;
						this.warnings.push(`Weapon ${info.weapon} couldn't be loaded.`);
					},
				),
			);
		}
		if (cs.deco && cs.decoDir !== null) {
			const dir = cs.decoDir;
			for (const item of cs.deco.items) {
				if (!item.file) continue;
				const path = dir ? `${dir}/${item.file}` : item.file;
				jobs.push(
					loadPropGlb(modelFileUrl(path), signal).then(
						(scene) => {
							if (signal.aborted || this.disposed) {
								disposeObject(scene);
								return;
							}
							this.mountDeco(scene, item, look);
						},
						(error: unknown) => {
							if (signal.aborted) throw error;
							this.warnings.push(`Prop ${item.prefab} couldn't be loaded.`);
						},
					),
				);
			}
		}
		await Promise.all(jobs);
	}

	private mountNode(name: string): Object3D | null {
		if (!name) return this.model.content;
		return this.model.content.getObjectByName(name) ?? null;
	}

	private attach(root: Object3D, node: Object3D, scale: number, look: Look) {
		root.scale.setScalar(scale);
		node.add(root);
		this.roots.push(root);
		applyShading(root, look);
	}

	private mountWeapon(scene: Group, info: WeaponInfo, look: Look): void {
		const mounts = info.mounts
			.map((m) => ({
				node: this.mountNode(m.node),
				scale: m.scale,
				name: m.node,
			}))
			.filter((m) => {
				if (!m.node) this.warnings.push(`Weapon mount ${m.name} is missing.`);
				return Boolean(m.node);
			});
		if (!mounts.length) {
			disposeObject(scene);
			return;
		}
		// Clone before shading: clones share geometry and materials, and the
		// shading cache on each mesh must not be copied (userData is
		// serialised by `clone`).
		const roots = mounts.map((_, i) => (i === 0 ? scene : scene.clone(true)));
		mounts.forEach((m, i) => {
			const root = roots[i];
			root.name = `weapon:${info.weapon}:${m.name}`;
			root.visible = false;
			this.attach(root, m.node as Object3D, m.scale, look);
			this.weapons.push({ root });
		});
		this.lastClip = undefined;
	}

	private mountDeco(scene: Group, item: DecoItem, look: Look): void {
		const node = this.mountNode(item.mount);
		if (!node) {
			this.warnings.push(`Prop mount ${item.mount} is missing.`);
			disposeObject(scene);
			return;
		}
		scene.name = `deco:${item.prefab}`;
		scene.visible = false;
		const baseline = new Map<Object3D, boolean>();
		scene.traverse((o) => baseline.set(o, o.visible));
		this.attach(scene, node, 1, look);
		this.decos.push({ item, root: scene, targets: new Map(), baseline });
		this.lastClip = undefined;
	}

	/**
	 * Objects a visibility path controls: the named node's subtree, plus
	 * every mesh of the same names elsewhere in the prop (the exporter puts
	 * skinned meshes at the scene root, outside the Unity hierarchy).
	 */
	private targetsFor(deco: MountedDeco, path: string): Object3D[] {
		const cached = deco.targets.get(path);
		if (cached) return cached;
		const name = nodeName(path);
		const named = deco.root.getObjectByName(name);
		const names = new Set<string>();
		named?.traverse((o) => {
			if (o.name) names.add(o.name);
		});
		const out: Object3D[] = [];
		if (named) out.push(named);
		else out.push(deco.root);
		deco.root.traverse((o) => {
			if (o !== named && (o as Mesh).isMesh && names.has(o.name)) out.push(o);
		});
		deco.targets.set(path, out);
		return out;
	}

	setEnabled(weapon: boolean, props: boolean): void {
		this.weaponOn = weapon;
		this.propsOn = props;
		this.lastClip = undefined;
	}

	setLook(look: Look): void {
		for (const root of this.roots) {
			applyShading(root, look);
		}
	}

	/**
	 * Applies the visibility for a clip time. Returns true when something
	 * changed (so the caller can request a frame).
	 */
	update(
		clip: string | null,
		source: ClipSource | null,
		time: number,
	): boolean {
		if (this.disposed) return false;
		if (clip === this.lastClip && time === this.lastTime) return false;
		this.lastClip = clip;
		this.lastTime = time;
		let changed = false;
		const cs = this.model.charscreen;
		if (cs?.weapon && this.weapons.length) {
			const on = this.weaponOn && weaponVisible(cs.weapon, clip, source, time);
			if (on !== this.lastWeaponVisible) {
				this.lastWeaponVisible = on;
				for (const w of this.weapons) w.root.visible = on;
				changed = true;
			}
		}
		if (cs?.deco) {
			for (const deco of this.decos) {
				const vis = this.propsOn
					? decoVisibility(cs.deco, deco.item, clip, time)
					: {};
				const paths = Object.keys(vis);
				const any = paths.some((p) => vis[p]);
				if (deco.root.visible !== any) {
					deco.root.visible = any;
					changed = true;
				}
				if (!any) continue;
				// Restore helper visibility, then apply the tracked nodes.
				for (const [o, v] of deco.baseline) {
					if (o !== deco.root && o.visible !== v) {
						o.visible = v;
						changed = true;
					}
				}
				for (const path of paths) {
					for (const o of this.targetsFor(deco, path)) {
						if (o === deco.root) continue;
						const v = vis[path] && (deco.baseline.get(o) ?? true);
						if (o.visible !== v) {
							o.visible = v;
							changed = true;
						}
					}
				}
			}
		}
		return changed;
	}

	dispose(): void {
		if (this.disposed) return;
		this.disposed = true;
		for (const root of this.roots) {
			root.parent?.remove(root);
			disposeShading(root);
		}
		// Clones share resources with the first weapon; dispose once.
		const seen = new Set<Object3D>();
		for (const root of this.roots) {
			if (seen.has(root)) continue;
			seen.add(root);
			if (root === this.weapons[0]?.root || !root.name.startsWith("weapon:")) {
				disposeObject(root);
			}
		}
		this.roots = [];
		this.weapons = [];
		this.decos = [];
	}
}
