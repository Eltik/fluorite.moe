/**
 * Studio display preferences, persisted per viewer in localStorage.
 * A tiny external store (useSyncExternalStore) so the canvas and the overlay
 * read the same values without prop drilling.
 */
import { useSyncExternalStore } from "react";

export const BACKGROUND_IDS = [
	"auto",
	"stage-light",
	"stage-dark",
	"black",
	"white",
	"grey",
	"custom",
] as const;
export type BackgroundId = (typeof BACKGROUND_IDS)[number];

export const LIGHTING_IDS = ["studio", "soft", "dramatic", "flat"] as const;
export type LightingId = (typeof LIGHTING_IDS)[number];

export type ZUpMode = "auto" | "on" | "off";

export const DPR_CAPS = [1, 1.5, 2, 3] as const;

export interface StudioPrefs {
	background: BackgroundId;
	customColor: string;
	lighting: LightingId;
	exposure: number;
	grid: boolean;
	groundShadow: boolean;
	wireframe: boolean;
	skeleton: boolean;
	autoRotate: boolean;
	stats: boolean;
	dprCap: number;
	zUp: ZUpMode;
	welcomeDismissed: boolean;
}

export const DEFAULT_PREFS: StudioPrefs = {
	background: "auto",
	customColor: "#2b2540",
	lighting: "studio",
	exposure: 1,
	grid: false,
	groundShadow: true,
	wireframe: false,
	skeleton: false,
	autoRotate: false,
	stats: false,
	dprCap: 2,
	zUp: "auto",
	welcomeDismissed: false,
};

const KEY = "fluorite.studio";

function pick<T extends string>(
	value: unknown,
	allowed: readonly T[],
	fallback: T,
): T {
	return typeof value === "string" &&
		(allowed as readonly string[]).includes(value)
		? (value as T)
		: fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
	return typeof value === "boolean" ? value : fallback;
}

export function isHexColor(value: unknown): value is string {
	return typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value);
}

function read(): StudioPrefs {
	try {
		const raw = window.localStorage.getItem(KEY);
		if (!raw) return DEFAULT_PREFS;
		const p = JSON.parse(raw) as Record<string, unknown>;
		const d = DEFAULT_PREFS;
		return {
			background: pick(p.background, BACKGROUND_IDS, d.background),
			customColor: isHexColor(p.customColor) ? p.customColor : d.customColor,
			lighting: pick(p.lighting, LIGHTING_IDS, d.lighting),
			exposure:
				typeof p.exposure === "number" &&
				p.exposure >= 0.25 &&
				p.exposure <= 2.5
					? p.exposure
					: d.exposure,
			grid: bool(p.grid, d.grid),
			groundShadow: bool(p.groundShadow, d.groundShadow),
			wireframe: bool(p.wireframe, d.wireframe),
			skeleton: bool(p.skeleton, d.skeleton),
			autoRotate: bool(p.autoRotate, d.autoRotate),
			stats: bool(p.stats, d.stats),
			dprCap:
				typeof p.dprCap === "number" &&
				(DPR_CAPS as readonly number[]).includes(p.dprCap)
					? p.dprCap
					: d.dprCap,
			zUp: pick(p.zUp, ["auto", "on", "off"] as const, d.zUp),
			welcomeDismissed: bool(p.welcomeDismissed, d.welcomeDismissed),
		};
	} catch {
		return DEFAULT_PREFS;
	}
}

let current: StudioPrefs | null = null;
const listeners = new Set<() => void>();

function snapshot(): StudioPrefs {
	if (current === null) current = read();
	return current;
}

export function getPrefs(): StudioPrefs {
	return snapshot();
}

export function setPrefs(patch: Partial<StudioPrefs>): void {
	current = { ...snapshot(), ...patch };
	try {
		window.localStorage.setItem(KEY, JSON.stringify(current));
	} catch {
		// Storage unavailable (private mode): keep the values in memory.
	}
	for (const listener of listeners) listener();
}

/** Restores every display preference except the dismissed welcome hint. */
export function resetPrefs(): void {
	setPrefs({ ...DEFAULT_PREFS, welcomeDismissed: snapshot().welcomeDismissed });
}

function subscribe(listener: () => void): () => void {
	listeners.add(listener);
	const onStorage = (event: StorageEvent) => {
		if (event.key !== KEY) return;
		current = read();
		listener();
	};
	window.addEventListener("storage", onStorage);
	return () => {
		listeners.delete(listener);
		window.removeEventListener("storage", onStorage);
	};
}

export function usePrefs(): StudioPrefs {
	return useSyncExternalStore(subscribe, snapshot, () => DEFAULT_PREFS);
}
