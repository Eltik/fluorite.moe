/** Studio keyboard shortcuts (documented in the help dialog). */
import * as React from "react";
import type { AnimationController } from "@/components/studio/animation";
import { getPrefs, setPrefs } from "@/components/studio/prefs";
import type { FlyoutId } from "@/components/studio/studio-app";

export interface ShortcutHandlers {
	enabled: boolean;
	uiHidden: boolean;
	onHelp: () => void;
	onToggleUi: () => void;
	onShowUi: () => void;
	/** Escape with the interface visible: closes a non-modal panel, if any. */
	onEscape: () => void;
	onReset: () => void;
	onLock: () => void;
	onFlyout: (flyout: FlyoutId) => void;
	onScreenshot: () => void;
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
};

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
