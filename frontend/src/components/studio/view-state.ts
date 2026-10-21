/**
 * Session-only view state (never persisted): interaction modes, the picked
 * mesh and the live camera readout. Two tiny external stores so the canvas,
 * the toolbar and the keyboard shortcuts read the same values.
 */
import { useSyncExternalStore } from "react";

export interface Selection {
	/** The parts entry the mesh belongs to (for isolate / hide); null for props. */
	partId: string | null;
	/** `Object3D.uuid` of the picked mesh. */
	meshId: string;
	/** Material slot hit on a multi-material mesh. */
	materialIndex: number;
}

export interface ViewState {
	/** One finger / left-drag pans instead of orbiting. */
	panMode: boolean;
	/** Click (tap) picks a mesh. */
	picking: boolean;
	/** Vertex normals helper (built on demand, so not persisted). */
	normals: boolean;
	selection: Selection | null;
	/** `Object3D.uuid` of the mesh the camera was last framed on, if any. */
	focus: string | null;
	/** The detail inspector for the selected mesh is open. */
	inspector: boolean;
}

const DEFAULT_VIEW: ViewState = {
	panMode: false,
	picking: false,
	normals: false,
	selection: null,
	focus: null,
	inspector: false,
};

let view: ViewState = DEFAULT_VIEW;
const viewListeners = new Set<() => void>();

export function getViewState(): ViewState {
	return view;
}

export function setViewState(patch: Partial<ViewState>): void {
	view = { ...view, ...patch };
	for (const l of viewListeners) l();
}

export function resetViewState(): void {
	setViewState(DEFAULT_VIEW);
}

function subscribeView(listener: () => void): () => void {
	viewListeners.add(listener);
	return () => viewListeners.delete(listener);
}

export function useViewState(): ViewState {
	return useSyncExternalStore(
		subscribeView,
		() => view,
		() => DEFAULT_VIEW,
	);
}

// ------------------------------------------------------------- camera

export interface CameraReadout {
	distance: number;
	minDistance: number;
	maxDistance: number;
	/** Vertical field of view, degrees. */
	fov: number;
}

let camera: CameraReadout | null = null;
const cameraListeners = new Set<() => void>();

/** Published by the stage whenever a rendered frame moved the camera. */
export function publishCamera(next: CameraReadout): void {
	const c = camera;
	if (
		c &&
		c.distance === next.distance &&
		c.minDistance === next.minDistance &&
		c.maxDistance === next.maxDistance &&
		c.fov === next.fov
	) {
		return;
	}
	camera = next;
	for (const l of cameraListeners) l();
}

function subscribeCamera(listener: () => void): () => void {
	cameraListeners.add(listener);
	return () => cameraListeners.delete(listener);
}

export function useCameraReadout(): CameraReadout | null {
	return useSyncExternalStore(
		subscribeCamera,
		() => camera,
		() => null,
	);
}
