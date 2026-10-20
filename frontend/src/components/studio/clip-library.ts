/**
 * Where the Studio gets its animation clips from.
 *
 * - File libraries: one animation-only GLB per clip under the model's
 *   `clips/<kind>/<id>/` folder, fetched on demand (selection or hover) and
 *   kept in a module-level cache keyed by URL. The cache dedupes in-flight
 *   requests, drops failures so a retry refetches, and caps itself so a long
 *   browsing session doesn't pin hundreds of clips.
 * - Embedded libraries: clips that came inside the model GLB (dev files,
 *   exports without an index). Already in memory, so "loading" is instant.
 */
import type { AnimationClip, Material, Mesh, Object3D } from "three";
import { MeshoptDecoder } from "three/examples/jsm/libs/meshopt_decoder.module.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import {
	type ClipEntry,
	clipStem,
	parseClipIndex,
	sourceFromName,
} from "@/lib/clips";

export interface ClipLibrary {
	readonly entries: readonly ClipEntry[];
	/** Resolves the clip, from cache when it was already fetched. */
	load(entry: ClipEntry): Promise<AnimationClip>;
	/** Starts fetching without waiting; errors are swallowed. */
	prefetch(entry: ClipEntry): void;
	/** Whether `load()` would resolve without a network round trip. */
	isReady(entry: ClipEntry): boolean;
}

// ------------------------------------------------------------- cache

/** Clips kept after they resolve; the oldest are evicted beyond this. */
const CACHE_LIMIT = 64;

const cache = new Map<string, Promise<AnimationClip>>();
const settled = new Set<string>();

let loader: GLTFLoader | null = null;
function getLoader(): GLTFLoader {
	if (!loader) {
		loader = new GLTFLoader();
		loader.setMeshoptDecoder(MeshoptDecoder);
	}
	return loader;
}

/** Clip GLBs are node trees without geometry; this covers dev files that aren't. */
function disposeScene(root: Object3D): void {
	root.traverse((node) => {
		const mesh = node as Mesh;
		if (!mesh.isMesh) return;
		mesh.geometry?.dispose();
		const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
		for (const m of mats as Material[]) m?.dispose();
	});
}

async function fetchClip(url: string): Promise<AnimationClip> {
	const res = await fetch(url);
	if (!res.ok) throw new Error(`Clip failed to load (${res.status})`);
	const buf = await res.arrayBuffer();
	const gltf = await getLoader().parseAsync(buf, "");
	// Only the tracks are kept; the scene is just the node tree they target.
	disposeScene(gltf.scene);
	const clip = gltf.animations[0];
	if (!clip) throw new Error("The clip file holds no animation.");
	return clip;
}

function touch(url: string): void {
	// Re-insert to mark as most recently used.
	const p = cache.get(url);
	if (!p) return;
	cache.delete(url);
	cache.set(url, p);
}

function evict(): void {
	if (cache.size <= CACHE_LIMIT) return;
	for (const url of cache.keys()) {
		if (cache.size <= CACHE_LIMIT) break;
		// Never evict something still in flight; its waiters hold the promise.
		if (!settled.has(url)) continue;
		cache.delete(url);
		settled.delete(url);
	}
}

/** The shared clip cache: dedupes in-flight loads, evicts failures. */
export function loadClipUrl(url: string): Promise<AnimationClip> {
	const hit = cache.get(url);
	if (hit) {
		touch(url);
		return hit;
	}
	const p = fetchClip(url).then(
		(clip) => {
			settled.add(url);
			return clip;
		},
		(error: unknown) => {
			if (cache.get(url) === p) cache.delete(url);
			throw error;
		},
	);
	cache.set(url, p);
	evict();
	return p;
}

export function isClipCached(url: string): boolean {
	return settled.has(url);
}

/** Test and debugging aid; the Studio never needs to clear it. */
export function clearClipCache(): void {
	cache.clear();
	settled.clear();
}

// ------------------------------------------------------------- libraries

/** Index entries plus the folder their GLBs live in. */
export function fileClipLibrary(
	base: string,
	entries: readonly ClipEntry[],
): ClipLibrary {
	const root = base.endsWith("/") ? base : `${base}/`;
	const urlOf = (entry: ClipEntry) =>
		`${root}${encodeURIComponent(entry.file)}`;
	return {
		entries,
		load: (entry) => loadClipUrl(urlOf(entry)),
		prefetch: (entry) => {
			const url = urlOf(entry);
			if (settled.has(url) || cache.has(url)) return;
			loadClipUrl(url).catch(() => {});
		},
		isReady: (entry) => settled.has(urlOf(entry)),
	};
}

/** Clips that arrived inside the model GLB. */
export function embeddedClipLibrary(
	clips: readonly AnimationClip[],
): ClipLibrary {
	const byFile = new Map<string, AnimationClip>();
	const entries: ClipEntry[] = [];
	for (const clip of clips) {
		const file = `${clip.name}.glb`;
		if (byFile.has(file)) continue;
		byFile.set(file, clip);
		entries.push({
			file,
			name: clip.name
				.replace(/^a_(?:actor|monster|npc)_[^_]+_/i, "")
				.replace(/_/g, " "),
			clip: clip.name,
			source: sourceFromName(clip.name),
			duration: clip.duration,
			loop: true,
		});
	}
	entries.sort((a, b) => a.name.localeCompare(b.name));
	return {
		entries,
		load: (entry) => {
			const clip = byFile.get(entry.file);
			return clip
				? Promise.resolve(clip)
				: Promise.reject(new Error(`Unknown clip ${clipStem(entry.file)}`));
		},
		prefetch: () => {},
		isReady: () => true,
	};
}

/** Fetches and validates a clip index; `base` is the folder of the GLBs. */
export async function loadClipIndex(
	indexUrl: string,
	signal: AbortSignal,
): Promise<ClipLibrary> {
	const res = await fetch(indexUrl, { signal });
	if (!res.ok) throw new Error(`Clip index failed to load (${res.status})`);
	const entries = parseClipIndex(await res.json());
	const base = indexUrl.slice(0, indexUrl.lastIndexOf("/") + 1);
	return fileClipLibrary(base, entries);
}
