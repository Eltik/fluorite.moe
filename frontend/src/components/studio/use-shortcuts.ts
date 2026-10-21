/** Studio keyboard shortcuts (documented in the help dialog). */
import * as React from "react";
import type { AnimationController } from "@/components/studio/animation";
import { getPrefs, SURFACE_IDS, setPrefs } from "@/components/studio/prefs";
import type { FlyoutId } from "@/components/studio/studio-app";
import {
	nextSurface,
	VIEW_PRESET_KEYS,
	type ViewPresetId,
	ZOOM_STEP,
} from "@/components/studio/view-math";
import { getViewState, setViewState } from "@/components/studio/view-state";

export interface ShortcutHandlers {
	enabled: boolean;
	uiHidden: boolean;
	onHelp: () => void;
	onToggleUi: () => void;
	onShowUi: () => void;
	/** Escape with the interface visible: closes a non-modal panel, if any. */
	onEscape: () => void;
	onReset: () => void;
	/** Shift + arrow keys: pan the view by a fraction of the frame. */
	onPan: (dx: number, dy: number) => void;
	/** = and −: multiply the camera distance. */
	onZoom: (factor: number) => void;
	/** Digits 1–6: view presets. */
	onView: (preset: ViewPresetId) => void;
	onLock: () => void;
	onFlyout: (flyout: FlyoutId) => void;
	onScreenshot: () => void;
	/** Backspace / Delete with a selection: hide that part. */
	onHideSelected: () => void;
	/** `.` with a selection: frame the camera on that mesh. */
	onFocusSelected: () => void;
	/** `/` with a selection: show only that part and frame it. */
	onIsolateFocusSelected: () => void;
	animation: AnimationController | null;
}

function isTyping(target: EventTarget | null): boolean {
	if (!(target instanceof HTMLElement)) return false;
	if (target.isContentEditable) return true;
	const tag = target.tagName;
	if (tag === "TEXTAREA" || tag === "SELECT") return true;
	if (tag === "INPUT") {
		const type = (target as HTMLInputElement).type;
		return !["checkbox", "radio", "range", "button", "color"].includes(type);
	}
	return false;
}

/** Keys that interactive widgets already use for themselves. */
function ownsArrows(target: EventTarget | null): boolean {
	return (
		target instanceof HTMLElement &&
		Boolean(
			target.closest(
				'[role="slider"],[role="tablist"],[role="radiogroup"],[data-tile],[role="group"]',
			),
		)
	);
}

const FLYOUT_KEYS: Record<string, FlyoutId> = {
	m: "models",
	p: "parts",
	a: "animations",
	b: "background",
	l: "lighting",
	v: "view",
};

const toggle = (key: "skeleton" | "bounds" | "axes" | "stats") =>
	setPrefs({ [key]: !getPrefs()[key] });

export function useStudioShortcuts(handlers: ShortcutHandlers): void {
	const ref = React.useRef(handlers);
	ref.current = handlers;

	React.useEffect(() => {
		const onKeyDown = (e: KeyboardEvent) => {
			const h = ref.current;
			if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
			if (isTyping(e.target)) return;
			if (e.key === "?") {
				e.preventDefault();
				h.onHelp();
				return;
			}
			if (!h.enabled) return;
			const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
			const anim = h.animation;
			if (e.shiftKey && key.startsWith("Arrow")) {
				if (ownsArrows(e.target)) return;
				const dx = key === "ArrowLeft" ? -1 : key === "ArrowRight" ? 1 : 0;
				const dy = key === "ArrowUp" ? -1 : key === "ArrowDown" ? 1 : 0;
				h.onPan(dx, dy);
				e.preventDefault();
				return;
			}
			// Shift + letter: the overlay / helper variants of the plain key.
			if (e.shiftKey && e.key.length === 1) {
				switch (key) {
					case "w":
						setPrefs({ wireframeOverlay: !getPrefs().wireframeOverlay });
						break;
					case "g":
						toggle("axes");
						break;
					case "s":
						toggle("stats");
						break;
					case "+":
					case "=":
						h.onZoom(ZOOM_STEP);
						break;
					case "_":
					case "-":
						h.onZoom(1 / ZOOM_STEP);
						break;
					default:
						return;
				}
				e.preventDefault();
				return;
			}
			const preset = VIEW_PRESET_KEYS[key];
			if (preset) {
				h.onView(preset);
				e.preventDefault();
				return;
			}
			switch (key) {
				case "h":
					h.onToggleUi();
					break;
				case "Escape":
					if (h.uiHidden) h.onShowUi();
					else h.onEscape();
					break;
				case "f":
					h.onReset();
					break;
				case "c":
					h.onLock();
					break;
				case "s":
					h.onScreenshot();
					break;
				case "g":
					setPrefs({ grid: !getPrefs().grid });
					break;
				case "o":
					setPrefs({ autoRotate: !getPrefs().autoRotate });
					break;
				case "w":
					setPrefs({ wireframe: !getPrefs().wireframe });
					break;
				case "t":
					setPrefs({
						shading: getPrefs().shading === "toon" ? "standard" : "toon",
					});
					break;
				case "u":
					setPrefs({ surface: nextSurface(SURFACE_IDS, getPrefs().surface) });
					break;
				case "k":
					toggle("skeleton");
					break;
				case "x":
					toggle("bounds");
					break;
				case "n":
					setViewState({ normals: !getViewState().normals });
					break;
				case "i":
					setViewState({ picking: !getViewState().picking });
					break;
				case "=":
				case "+":
					h.onZoom(ZOOM_STEP);
					break;
				case "-":
					h.onZoom(1 / ZOOM_STEP);
					break;
				case "Backspace":
				case "Delete":
					if (!getViewState().selection) return;
					h.onHideSelected();
					break;
				case ".":
					if (!getViewState().selection) return;
					h.onFocusSelected();
					break;
				case "/":
					if (!getViewState().selection) return;
					h.onIsolateFocusSelected();
					break;
				case " ":
					if (!anim || e.target instanceof HTMLButtonElement) return;
					anim.toggle();
					break;
				case "ArrowLeft":
				case "ArrowRight":
					if (!anim || ownsArrows(e.target)) return;
					anim.step(key === "ArrowLeft" ? -1 : 1);
					break;
				case "r":
					if (!anim) return;
					anim.restart();
					break;
				default: {
					const flyout = FLYOUT_KEYS[key];
					if (!flyout) return;
					h.onFlyout(flyout);
				}
			}
			e.preventDefault();
		};
		window.addEventListener("keydown", onKeyDown);
		return () => window.removeEventListener("keydown", onKeyDown);
	}, []);
}
