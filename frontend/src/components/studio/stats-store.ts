/** Renderer statistics published by the canvas for the stats overlay. */
import { useSyncExternalStore } from "react";

export interface RenderStats {
	fps: number;
	calls: number;
	triangles: number;
	geometries: number;
	textures: number;
	dpr: number;
	width: number;
	height: number;
}

let stats: RenderStats | null = null;
const listeners = new Set<() => void>();

export function publishStats(next: RenderStats): void {
	stats = next;
	for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
	listeners.add(listener);
	return () => listeners.delete(listener);
}

export function useRenderStats(): RenderStats | null {
	return useSyncExternalStore(
		subscribe,
		() => stats,
		() => null,
	);
}
