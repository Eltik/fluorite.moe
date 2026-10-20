/**
 * Model loading for the Studio.
 *
 * - Rigged: one skinned GLB with embedded textures (and possibly clips).
 * - Static parts: every part GLB is fetched in parallel (byte progress), parsed
 *   with GLTFLoader and given a material. Diffuse textures are bound by naming
 *   convention: `S_<stem>_lodN.glb` → `T_<stem>_D.png` in the sibling
 *   `textures/` folder, falling back to the closest `_D` texture of the same
 *   part, then to a neutral material.
 */
import {
	type AnimationClip,
	Box3,
	DoubleSide,
	Group,
	type Material,
	type Mesh,
	MeshStandardMaterial,
	type Object3D,
	type SkinnedMesh,
	SRGBColorSpace,
	Texture,
	Vector3,
} from "three";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { fetchBytes, ProgressTracker } from "@/components/studio/bytes";
import type { StudioModelSource } from "@/components/studio/catalog-source";
import { disposeShading } from "@/components/studio/shading";
import { api, fileUrl, modelFileUrl, type TreeEntry } from "@/lib/api";
import { basename, dirname } from "@/lib/format";

export type PartStatus = "textured" | "shared" | "neutral" | "embedded";

export interface PartEntry {
	id: string;
	label: string;
	/** Source GLB path (static parts). */
	path?: string;
	lod?: number;
	object: Object3D;
	status: PartStatus;
	/** Diffuse texture path under the `textures` root. */
	texture?: string;
	triangles: number;
	hiddenByDefault: boolean;
}

export interface ModelStats {
	meshes: number;
	triangles: number;
	textures: number;
	bytes: number;
}

export interface LoadedModel {
	key: string;
	name: string;
	rigged: boolean;
	/** The raw content, in file coordinates. */
	content: Object3D;
	parts: PartEntry[];
	animations: AnimationClip[];
	/** True when the bounds suggest the file is Z-up. */
	detectedZUp: boolean;
	stats: ModelStats;
	/** Non-fatal problems (missing parts, textures that failed to decode). */
	warnings: string[];
	dispose: () => void;
}

export interface LoadOptions {
	signal: AbortSignal;
	onProgress: (loaded: number, total: number) => void;
}

const NEUTRAL_COLOR = 0xb9b5c6;

function makeLoader(): GLTFLoader {
	const loader = new GLTFLoader();
	loader.setMeshoptDecoder(MeshoptDecoder);
	return loader;
}

function throwIfAborted(signal: AbortSignal): void {
	if (signal.aborted) throw new DOMException("Aborted", "AbortError");
}

function triangleCount(object: Object3D): number {
	let tris = 0;
	object.traverse((node) => {
		const mesh = node as Mesh;
		if (!mesh.isMesh) return;
		const g = mesh.geometry;
		tris += Math.floor(
			(g.index ? g.index.count : (g.attributes.position?.count ?? 0)) / 3,
		);
	});
	return tris;
}

function collectTextures(material: Material, into: Set<Texture>): void {
	for (const value of Object.values(material)) {
		if (value instanceof Texture) into.add(value);
	}
}

/** Frees GPU resources (geometries, materials, textures, bitmaps). */
export function disposeObject(root: Object3D): void {
	const textures = new Set<Texture>();
	const materials = new Set<Material>();
	root.traverse((node) => {
		const mesh = node as Mesh;
		if (mesh.isMesh) {
			mesh.geometry?.dispose();
			const mats = Array.isArray(mesh.material)
				? mesh.material
				: [mesh.material];
			for (const m of mats) if (m) materials.add(m);
		}
		if ((node as SkinnedMesh).isSkinnedMesh) {
			(node as SkinnedMesh).skeleton?.dispose();
		}
	});
	for (const m of materials) {
		collectTextures(m, textures);
		m.dispose();
	}
	for (const t of textures) {
		t.dispose();
		const data = t.source?.data as { close?: () => void } | undefined;
		if (data && typeof data.close === "function") data.close();
	}
}

// ------------------------------------------------------------- naming

const MAP_SUFFIX = /_(d|n|p|e|st|m|hn|sw_m)$/i;

function stem(path: string): string {
	return basename(path).replace(/\.(glb|png)$/i, "");
}

/** `.../S_actor_chen_cloth_01_lod1.glb` → `actor_chen_cloth_01`. */
export function partKey(path: string): string {
	return stem(path)
		.replace(/^S_/i, "")
		.replace(/_lod\d+$/i, "")
		.toLowerCase();
}

function lodOf(path: string): number | undefined {
	const m = /_lod(\d+)\.glb$/i.exec(path);
	return m ? Number(m[1]) : undefined;
}

/** `T_actor_chen_body_01_D.png` → `actor_chen_body_01`, or null for non-diffuse maps. */
function diffuseKey(path: string): string | null {
	const s = stem(path).replace(/^T_/i, "");
	const m = MAP_SUFFIX.exec(s);
	if (!m || m[1].toLowerCase() !== "d") return null;
	return s.slice(0, m.index).toLowerCase();
}

function tokens(key: string): { words: string[]; nums: string[] } {
	const words: string[] = [];
	const nums: string[] = [];
	for (const t of key.split("_")) {
		const n = /^(\d+)[a-z]?$/.exec(t);
		if (n) nums.push(String(Number(n[1])));
		else if (t) words.push(t);
	}
	return { words, nums };
}

interface TextureCandidate {
	path: string;
	key: string;
	size: number;
}

/** Best diffuse texture for a part, or null. `exact` = same part and slot. */
export function matchTexture(
	part: string,
	candidates: TextureCandidate[],
): { path: string; exact: boolean } | null {
	const p = tokens(part);
	let best: { path: string; exact: boolean; score: number } | null = null;
	for (const c of candidates) {
		if (c.key === part) return { path: c.path, exact: true };
		const t = tokens(c.key);
		if (!p.words.every((w) => t.words.includes(w))) continue;
		const extra = t.words.filter((w) => !p.words.includes(w)).length;
		const sameNums = p.nums.join("_") === t.nums.join("_");
		const firstNum = p.nums[0] === t.nums[0];
		const score =
			50 +
			(sameNums ? 30 : firstNum ? 10 : 0) -
			extra * 5 -
			(t.nums.length ? Number(t.nums.at(-1)) : 0) * 0.01;
		if (!best || score > best.score) {
			best = { path: c.path, exact: sameNums && extra <= 1, score };
		}
	}
	return best ? { path: best.path, exact: best.exact } : null;
}

/** Parts that are rendering helpers in-game (hair shadow proxies etc.). */
function isHelperPart(key: string): boolean {
	const words = tokens(key).words;
	return (
		words.includes("hs") ||
		words.includes("eyeshadow") ||
		words.includes("shadow")
	);
}

function prettyLabel(key: string, common: string): string {
	const rest = key.startsWith(common) ? key.slice(common.length) : key;
	return (rest || key).replace(/_/g, " ").trim();
}

function commonPrefix(keys: string[]): string {
	if (keys.length < 2) return "";
	let prefix = keys[0];
	for (const k of keys) {
		while (!k.startsWith(prefix)) prefix = prefix.slice(0, -1);
	}
	// Cut back to a token boundary.
	const cut = prefix.lastIndexOf("_");
	return cut >= 0 ? prefix.slice(0, cut + 1) : "";
}

// ------------------------------------------------------------- textures

async function listPngs(
	dir: string,
	signal: AbortSignal,
): Promise<TreeEntry[]> {
	try {
		const page = await api.tree("textures", { path: dir, limit: 2000 }, signal);
		return page.entries.filter(
			(e) => e.kind === "file" && e.name.toLowerCase().endsWith(".png"),
		);
	} catch (error) {
		if (signal.aborted) throw error;
		return [];
	}
}

/** Finds `_D` texture candidates for a set of part paths. */
async function findTextureCandidates(
	parts: string[],
	signal: AbortSignal,
): Promise<TextureCandidate[]> {
	const dirs = [...new Set(parts.map((p) => dirname(p)))];
	const found = new Map<string, TextureCandidate>();
	const add = (path: string, size: number) => {
		const key = diffuseKey(path);
		if (key && !found.has(path)) found.set(path, { path, key, size });
	};
	await Promise.all(
		dirs.map(async (dir) => {
			const guesses = dir.endsWith("/models")
				? [`${dir.slice(0, -"/models".length)}/textures`, dir]
				: [`${dir}/textures`, dir];
			for (const guess of guesses) {
				const files = await listPngs(guess, signal);
				for (const f of files) add(f.path, f.size ?? 0);
				if (files.length) return;
			}
		}),
	);
	if (found.size) return [...found.values()];
	// Path guess failed: search by the entity folder name.
	const codes = new Set(
		dirs.map((d) => basename(d.endsWith("/models") ? dirname(d) : d)),
	);
	for (const code of codes) {
		if (!code) continue;
		try {
			const res = await api.search(
				{ q: code, root: "textures", ext: "png", limit: 200 },
				signal,
			);
			for (const item of res.items) {
				if (/^T_/i.test(item.name)) add(item.path, item.size);
			}
		} catch (error) {
			if (signal.aborted) throw error;
		}
	}
	return [...found.values()];
}

async function loadTexture(
	path: string,
	options: { signal: AbortSignal; onProgress: (l: number, t: number) => void },
): Promise<Texture> {
	const buf = await fetchBytes(fileUrl("textures", path), options);
	throwIfAborted(options.signal);
	const bitmap = await createImageBitmap(
		new Blob([buf], { type: "image/png" }),
		{
			premultiplyAlpha: "none",
			colorSpaceConversion: "none",
		},
	);
	const texture = new Texture(bitmap);
	texture.flipY = false;
	texture.colorSpace = SRGBColorSpace;
	texture.anisotropy = 8;
	texture.name = basename(path);
	texture.needsUpdate = true;
	return texture;
}

// ------------------------------------------------------------- loaders

function detectZUp(content: Object3D): boolean {
	const box = new Box3().setFromObject(content);
	if (box.isEmpty()) return false;
	const size = box.getSize(new Vector3());
	return size.z > size.y * 1.15;
}

async function loadRigged(
	source: StudioModelSource,
	key: string,
	{ signal, onProgress }: LoadOptions,
): Promise<LoadedModel> {
	const path = source.rigged as string;
	const tracker = new ProgressTracker(onProgress);
	try {
		const buf = await fetchBytes(modelFileUrl(path), {
			signal,
			onProgress: tracker.reporter(path),
		});
		throwIfAborted(signal);
		const gltf = await makeLoader().parseAsync(buf, "");
		if (signal.aborted) {
			disposeObject(gltf.scene);
			throwIfAborted(signal);
		}
		const content = gltf.scene;
		const parts: PartEntry[] = [];
		const textures = new Set<Texture>();
		content.traverse((node) => {
			const mesh = node as Mesh;
			if (!mesh.isMesh) return;
			if ((node as SkinnedMesh).isSkinnedMesh) mesh.frustumCulled = false;
			const mats = Array.isArray(mesh.material)
				? mesh.material
				: [mesh.material];
			for (const m of mats) if (m) collectTextures(m, textures);
			parts.push({
				id: mesh.uuid,
				label: mesh.name || `Mesh ${parts.length + 1}`,
				object: mesh,
				status: "embedded",
				triangles: triangleCount(mesh),
				hiddenByDefault: false,
			});
		});
		return {
			key,
			name: source.entry.name,
			rigged: true,
			content,
			parts,
			animations: gltf.animations,
			detectedZUp: detectZUp(content),
			stats: {
				meshes: parts.length,
				triangles: parts.reduce((s, p) => s + p.triangles, 0),
				textures: textures.size,
				bytes: buf.byteLength,
			},
			warnings: [],
			dispose: () => {
				disposeShading(content);
				disposeObject(content);
			},
		};
	} finally {
		tracker.dispose();
	}
}

async function loadParts(
	source: StudioModelSource,
	key: string,
	{ signal, onProgress }: LoadOptions,
): Promise<LoadedModel> {
	const tracker = new ProgressTracker(onProgress);
	const loader = makeLoader();
	const warnings: string[] = [];
	const created: Object3D[] = [];
	const textureCache = new Map<string, Promise<Texture | null>>();
	let bytes = 0;

	try {
		const candidatesPromise = findTextureCandidates(source.parts, signal);
		const meshesPromise = Promise.allSettled(
			source.parts.map(async (path) => {
				const buf = await fetchBytes(fileUrl("meshes", path), {
					signal,
					onProgress: tracker.reporter(path),
				});
				throwIfAborted(signal);
				bytes += buf.byteLength;
				const gltf = await loader.parseAsync(buf, "");
				// The catch below may already have run for this abort, so a
				// late scene is disposed here rather than left in `created`.
				if (signal.aborted) {
					disposeObject(gltf.scene);
					throwIfAborted(signal);
				}
				created.push(gltf.scene);
				return { path, scene: gltf.scene };
			}),
		);

		const candidates = await candidatesPromise;
		const matches = new Map<string, { path: string; exact: boolean }>();
		for (const path of source.parts) {
			const match = matchTexture(partKey(path), candidates);
			if (match) matches.set(path, match);
		}
		const sizes = new Map(candidates.map((c) => [c.path, c.size]));
		for (const m of new Set([...matches.values()].map((m) => m.path))) {
			tracker.expect(m, sizes.get(m) ?? 0);
			textureCache.set(
				m,
				loadTexture(m, { signal, onProgress: tracker.reporter(m) }).then(
					(texture) => {
						bytes += sizes.get(m) ?? 0;
						return texture;
					},
					(error: unknown) => {
						if (signal.aborted) throw error;
						warnings.push(`Texture ${basename(m)} failed to load`);
						return null;
					},
				),
			);
		}

		const [meshResults, textureEntries] = await Promise.all([
			meshesPromise,
			Promise.all(
				[...textureCache].map(async ([p, t]) => [p, await t] as const),
			),
		]);
		throwIfAborted(signal);
		const textures = new Map(textureEntries);

		const content = new Group();
		content.name = source.entry.name;
		const keys = source.parts.map(partKey);
		const common = commonPrefix(keys);
		const parts: PartEntry[] = [];
		const materials = new Map<string, MeshStandardMaterial>();

		const neutral = () =>
			new MeshStandardMaterial({
				color: NEUTRAL_COLOR,
				roughness: 0.55,
				metalness: 0,
				side: DoubleSide,
			});

		meshResults.forEach((result, i) => {
			const path = source.parts[i];
			if (result.status === "rejected") {
				if (signal.aborted) return;
				warnings.push(
					`${basename(path)}: ${String((result.reason as Error)?.message ?? result.reason)}`,
				);
				return;
			}
			const key = keys[i];
			const match = matches.get(path);
			const texture = match ? (textures.get(match.path) ?? null) : null;
			let material: MeshStandardMaterial;
			if (texture && match) {
				let shared = materials.get(match.path);
				if (!shared) {
					shared = new MeshStandardMaterial({
						map: texture,
						roughness: 0.8,
						metalness: 0,
						side: DoubleSide,
					});
					shared.name = basename(match.path);
					materials.set(match.path, shared);
				}
				material = shared;
			} else {
				material = neutral();
			}
			const scene = result.value.scene;
			scene.traverse((node) => {
				const mesh = node as Mesh;
				if (!mesh.isMesh) return;
				const old = mesh.material;
				for (const m of Array.isArray(old) ? old : [old]) m?.dispose();
				mesh.material = material;
			});
			scene.name = key;
			content.add(scene);
			parts.push({
				id: path,
				label: prettyLabel(key, common),
				path,
				lod: lodOf(path),
				object: scene,
				status: texture ? (match?.exact ? "textured" : "shared") : "neutral",
				texture: texture ? match?.path : undefined,
				triangles: triangleCount(scene),
				hiddenByDefault: isHelperPart(key),
			});
		});

		// Release textures that ended up unused (e.g. their part failed).
		for (const [p, t] of textures) {
			if (t && !materials.has(p)) {
				t.dispose();
				(t.source.data as ImageBitmap | undefined)?.close?.();
			}
		}

		if (!parts.length) {
			throw new Error(warnings[0] ?? "This model has no loadable parts.");
		}

		return {
			key,
			name: source.entry.name,
			rigged: false,
			content,
			parts,
			animations: [],
			detectedZUp: detectZUp(content),
			stats: {
				meshes: parts.length,
				triangles: parts.reduce((s, p) => s + p.triangles, 0),
				textures: materials.size,
				bytes,
			},
			warnings,
			dispose: () => {
				disposeShading(content);
				disposeObject(content);
			},
		};
	} catch (error) {
		for (const obj of created) disposeObject(obj);
		for (const t of textureCache.values()) {
			void t.then(
				(tex) => {
					tex?.dispose();
					(tex?.source.data as ImageBitmap | undefined)?.close?.();
				},
				() => {},
			);
		}
		throw error;
	} finally {
		tracker.dispose();
	}
}

export function loadModel(
	source: StudioModelSource,
	options: LoadOptions,
): Promise<LoadedModel> {
	const key = `${source.entry.kind}/${source.entry.id}`;
	if (source.rigged) return loadRigged(source, key, options);
	if (!source.parts.length) {
		return Promise.reject(
			new Error("No 3D model was exported for this entry."),
		);
	}
	return loadParts(source, key, options);
}
