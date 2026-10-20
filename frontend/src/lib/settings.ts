/**
 * Per-viewer settings (theme, text language, UI density) persisted in
 * localStorage. The pre-hydration script in `__root.tsx` applies the theme and
 * density classes before first paint; this store keeps them in sync afterwards.
 */
import { useSyncExternalStore } from "react";

export type ThemePreference = "light" | "dark" | "system";
export type Density = "comfortable" | "compact";

export interface Settings {
	theme: ThemePreference;
	textLanguage: string;
	density: Density;
}

export const SETTINGS_KEY = "fluorite.settings";

export const DEFAULT_SETTINGS: Settings = {
	theme: "system",
	textLanguage: "EN",
	density: "comfortable",
};

function read(): Settings {
	try {
		const raw = window.localStorage.getItem(SETTINGS_KEY);
		if (!raw) return DEFAULT_SETTINGS;
		const parsed = JSON.parse(raw) as Partial<Settings>;
		return {
			theme:
				parsed.theme === "light" ||
				parsed.theme === "dark" ||
				parsed.theme === "system"
					? parsed.theme
					: DEFAULT_SETTINGS.theme,
			textLanguage:
				typeof parsed.textLanguage === "string" && parsed.textLanguage
					? parsed.textLanguage
					: DEFAULT_SETTINGS.textLanguage,
			density:
				parsed.density === "compact" ? "compact" : DEFAULT_SETTINGS.density,
		};
	} catch {
		return DEFAULT_SETTINGS;
	}
}

let current: Settings | null = null;
const listeners = new Set<() => void>();

function snapshot(): Settings {
	if (current === null) current = read();
	return current;
}

/** BCP-47 tags for the game's text languages (`html[lang]` follows the text). */
export const LANGUAGE_TAGS: Record<string, string> = {
	EN: "en",
	CN: "zh-Hans",
	TC: "zh-Hant",
	JP: "ja",
	KR: "ko",
	BR: "pt-BR",
	DE: "de",
	FR: "fr",
	ID: "id",
	IT: "it",
	MX: "es-MX",
	RU: "ru",
	TH: "th",
	VN: "vi",
};

export function languageTag(code: string): string {
	return LANGUAGE_TAGS[code] ?? "en";
}

function prefersDark(): boolean {
	return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function resolveTheme(theme: ThemePreference): "light" | "dark" {
	if (theme === "system") return prefersDark() ? "dark" : "light";
	return theme;
}

function apply(settings: Settings): void {
	const root = document.documentElement;
	const dark = resolveTheme(settings.theme) === "dark";
	root.classList.toggle("dark", dark);
	root.style.colorScheme = dark ? "dark" : "light";
	root.dataset.density = settings.density;
	root.lang = languageTag(settings.textLanguage);
}

export function updateSettings(patch: Partial<Settings>): void {
	const next = { ...snapshot(), ...patch };
	current = next;
	try {
		window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
	} catch {
		// Storage can be unavailable (private mode); settings stay in memory.
	}
	apply(next);
	for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
	listeners.add(listener);
	const media = window.matchMedia("(prefers-color-scheme: dark)");
	const onScheme = () => {
		if (snapshot().theme === "system") apply(snapshot());
		listener();
	};
	const onStorage = (event: StorageEvent) => {
		if (event.key !== SETTINGS_KEY) return;
		current = read();
		apply(current);
		listener();
	};
	media.addEventListener("change", onScheme);
	window.addEventListener("storage", onStorage);
	return () => {
		listeners.delete(listener);
		media.removeEventListener("change", onScheme);
		window.removeEventListener("storage", onStorage);
	};
}

export function useSettings(): Settings {
	return useSyncExternalStore(subscribe, snapshot, () => DEFAULT_SETTINGS);
}

/** Row height in px for virtualized lists, by density. */
export function useRowHeight(): number {
	return useSettings().density === "compact" ? 28 : 34;
}

/**
 * Inline script run in <head> before hydration. Kept as a string so it can be
 * injected verbatim; must stay in sync with `apply()` above.
 */
export const PREHYDRATION_SCRIPT = `(function(){try{var s=JSON.parse(localStorage.getItem(${JSON.stringify(
	SETTINGS_KEY,
)})||"{}");var t=s.theme;var d=t==="dark"||((!t||t==="system")&&matchMedia("(prefers-color-scheme: dark)").matches);var r=document.documentElement;if(d)r.classList.add("dark");r.style.colorScheme=d?"dark":"light";r.dataset.density=s.density==="compact"?"compact":"comfortable";var l=${JSON.stringify(
	LANGUAGE_TAGS,
)}[s.textLanguage];if(l)r.lang=l;}catch(e){}})();`;
