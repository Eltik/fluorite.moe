/**
 * One shared `<audio>` element for the whole app: starting a clip anywhere
 * (the audio browser, an `<AudioButton>` in a voice tab) stops the previous
 * one. Components read the state with `useAudioState()`.
 */
import * as React from "react";
import { type AudioSources, audioSrc } from "@/lib/audio";

export interface AudioTrack {
	/** Stable key, used to tell which button / row is the current one. */
	key: string;
	src: AudioSources | string;
	title?: string;
	subtitle?: string;
}

export type AudioStatus = "idle" | "loading" | "playing" | "paused" | "error";

export interface AudioState {
	track: AudioTrack | null;
	status: AudioStatus;
	time: number;
	duration: number;
	error?: string;
}

const IDLE: AudioState = { track: null, status: "idle", time: 0, duration: 0 };

let state: AudioState = IDLE;
let element: HTMLAudioElement | null = null;
let loadTimer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<() => void>();

/** A clip still buffering after this long is reported as failed. */
const LOAD_TIMEOUT_MS = 20_000;
const endedListeners = new Set<(track: AudioTrack) => void>();

function set(patch: Partial<AudioState>): void {
	state = { ...state, ...patch };
	if (state.status !== "loading" && loadTimer) {
		clearTimeout(loadTimer);
		loadTimer = undefined;
	}
	for (const l of listeners) l();
}

function audio(): HTMLAudioElement {
	if (element) return element;
	const el = new Audio();
	el.preload = "auto";
	el.addEventListener("playing", () => set({ status: "playing" }));
	el.addEventListener("waiting", () => set({ status: "loading" }));
	el.addEventListener("pause", () => {
		if (state.status !== "error" && !el.ended) set({ status: "paused" });
	});
	el.addEventListener("timeupdate", () => set({ time: el.currentTime }));
	el.addEventListener("durationchange", () =>
		set({ duration: Number.isFinite(el.duration) ? el.duration : 0 }),
	);
	el.addEventListener("ended", () => {
		set({ status: "paused", time: el.duration || state.time });
		const track = state.track;
		if (track) for (const l of endedListeners) l(track);
	});
	el.addEventListener("error", () => {
		if (!el.currentSrc && !el.src) return;
		set({
			status: "error",
			error:
				el.error?.code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED
					? "This clip could not be decoded"
					: "Playback failed",
		});
	});
	element = el;
	return el;
}

export const audioEngine = {
	getState: (): AudioState => state,

	subscribe(listener: () => void): () => void {
		listeners.add(listener);
		return () => listeners.delete(listener);
	},

	/** Called when a clip plays to the end (for auto-advance). */
	onEnded(listener: (track: AudioTrack) => void): () => void {
		endedListeners.add(listener);
		return () => endedListeners.delete(listener);
	},

	play(track: AudioTrack): void {
		const url = audioSrc(track.src);
		const el = audio();
		if (!url) {
			const keep = state.track?.key === track.key ? state.error : undefined;
			set({ track, status: "error", error: keep ?? "No playable source" });
			return;
		}
		if (
			state.track?.key === track.key &&
			// `el.src` is always absolute; `url` is relative when API_URL is "".
			el.src === new URL(url, location.href).href &&
			state.status !== "error"
		) {
			if (el.ended) el.currentTime = 0;
			void el.play().catch(() => {});
			return;
		}
		set({ track, status: "loading", time: 0, duration: 0, error: undefined });
		clearTimeout(loadTimer);
		loadTimer = setTimeout(() => {
			if (state.track?.key === track.key && state.status === "loading") {
				el.pause();
				set({ status: "error", error: "Timed out loading this clip" });
			}
		}, LOAD_TIMEOUT_MS);
		el.src = url;
		void el.play().catch((e: unknown) => {
			// AbortError: superseded by another play() call.
			if (e instanceof DOMException && e.name === "AbortError") return;
			if (state.track?.key === track.key && state.status !== "error")
				set({ status: "error", error: "Playback was blocked" });
		});
	},

	/**
	 * Make `track` current without playing it and explain why (e.g. a
	 * soundbank ref with no decoded media), so the player bar says something
	 * instead of silently ignoring the click.
	 */
	unavailable(track: AudioTrack, message: string): void {
		if (element && !element.paused) element.pause();
		set({ track, status: "error", time: 0, duration: 0, error: message });
	},

	pause(): void {
		element?.pause();
	},

	/** Play `track`, or pause / resume it when it is already current. */
	toggle(track?: AudioTrack): void {
		const current = state.track;
		if (track && current?.key !== track.key) {
			audioEngine.play(track);
			return;
		}
		if (!current) return;
		if (state.status === "playing" || state.status === "loading") {
			audioEngine.pause();
		} else {
			audioEngine.play(current);
		}
	},

	seek(seconds: number): void {
		const el = element;
		if (!el || !state.track) return;
		const max = Number.isFinite(el.duration) ? el.duration : seconds;
		el.currentTime = Math.max(0, Math.min(max, seconds));
		set({ time: el.currentTime });
	},

	stop(): void {
		if (element) {
			element.pause();
			element.removeAttribute("src");
			element.load();
		}
		set(IDLE);
	},
};

export function useAudioState(): AudioState {
	return React.useSyncExternalStore(
		audioEngine.subscribe,
		audioEngine.getState,
		() => IDLE,
	);
}

/** Whether `key` is the current track, and if so whether it is audible. */
export function useTrackStatus(key: string): AudioStatus {
	const s = useAudioState();
	return s.track?.key === key ? s.status : "idle";
}
