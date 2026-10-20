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

export const SHADING_IDS = ["toon", "standard"] as const;
export type ShadingId = (typeof SHADING_IDS)[number];

export const PICKER_KINDS = [
	"operators",
	"enemies",
	"weapons",
	"npcs",
] as const;
export type PickerKind = (typeof PICKER_KINDS)[number];

export type ZUpMode = "auto" | "on" | "off";

export const DPR_CAPS = [1, 1.5, 2, 3] as const;

export interface StudioPrefs {
	background: BackgroundId;
	customColor: string;
	lighting: LightingId;
	shading: ShadingId;
	outlines: boolean;
	exposure: number;
	grid: boolean;
	floor: boolean;
	groundShadow: boolean;
	wireframe: boolean;
	skeleton: boolean;
	autoRotate: boolean;
	stats: boolean;
	/** Toon mode: the key light follows the camera (else the front view). */
	lightFollow: boolean;
	/** Toon mode: soft bloom post pass. */
	bloom: boolean;
	/** Toon mode: the character's additional wrap / rim lights. */
	extraLights: boolean;
	/** Toon mode: the character shadows itself (and the floor). */
	selfShadow: boolean;
	dprCap: number;
	zUp: ZUpMode;
	welcomeDismissed: boolean;
	/** Last tab opened in the model picker. */
	pickerKind: PickerKind;
}

export const DEFAULT_PREFS: StudioPrefs = {
	background: "auto",
	customColor: "#2b2540",
	lighting: "studio",
	shading: "toon",
	outlines: true,
	exposure: 1,
	grid: false,
	floor: true,
	groundShadow: true,
	wireframe: false,
	skeleton: false,
	autoRotate: false,
	stats: false,
	lightFollow: true,
	bloom: false,
	extraLights: true,
	selfShadow: true,
	dprCap: 2,
	zUp: "auto",
	welcomeDismissed: false,
	pickerKind: "operators",
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
			shading: pick(p.shading, SHADING_IDS, d.shading),
			outlines: bool(p.outlines, d.outlines),
			exposure:
				typeof p.exposure === "number" &&
				p.exposure >= 0.25 &&
				p.exposure <= 2.5
					? p.exposure
					: d.exposure,
			grid: bool(p.grid, d.grid),
			floor: bool(p.floor, d.floor),
			groundShadow: bool(p.groundShadow, d.groundShadow),
			wireframe: bool(p.wireframe, d.wireframe),
			skeleton: bool(p.skeleton, d.skeleton),
			autoRotate: bool(p.autoRotate, d.autoRotate),
			stats: bool(p.stats, d.stats),
			lightFollow: bool(p.lightFollow, d.lightFollow),
			bloom: bool(p.bloom, d.bloom),
			extraLights: bool(p.extraLights, d.extraLights),
			selfShadow: bool(p.selfShadow, d.selfShadow),
			dprCap:
				typeof p.dprCap === "number" &&
				(DPR_CAPS as readonly number[]).includes(p.dprCap)
					? p.dprCap
					: d.dprCap,
			zUp: pick(p.zUp, ["auto", "on", "off"] as const, d.zUp),
			welcomeDismissed: bool(p.welcomeDismissed, d.welcomeDismissed),
			pickerKind: pick(p.pickerKind, PICKER_KINDS, d.pickerKind),
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

/** Restores every display preference except the welcome hint and picker tab. */
export function resetPrefs(): void {
	const { welcomeDismissed, pickerKind } = snapshot();
	setPrefs({ ...DEFAULT_PREFS, welcomeDismissed, pickerKind });
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
