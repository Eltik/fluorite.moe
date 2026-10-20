/**
 * AnimationMixer wrapper with a subscribable state for the transport UI and
 * the animations panel. Clips come from a `ClipLibrary` and are fetched when
 * selected; selection crossfades from whatever is playing. The canvas drives
 * `update()` from useFrame while playing; everything else is imperative so
 * the overlay can scrub without re-rendering the scene graph.
 */
import {
	type AnimationAction,
	AnimationClip,
	AnimationMixer,
	LoopOnce,
	LoopRepeat,
	type Object3D,
	VectorKeyframeTrack,
} from "three";
import type { ClipLibrary } from "@/components/studio/clip-library";
import { type ClipEntry, clipStem, defaultClip } from "@/lib/clips";

export const FRAME = 1 / 30;
export const SPEEDS = [0.25, 0.5, 1, 1.5, 2] as const;
/** Crossfade between clips, in clip seconds. */
export const FADE = 0.25;

export interface AnimationState {
	/** Stem of the selected clip file (also the `?clip=` value). */
	clip: string | null;
	/** Stem of the clip being fetched, while a selection is pending. */
	loading: string | null;
	/** Stem of the last clip that failed to load. */
	failed: string | null;
	playing: boolean;
	speed: number;
	loop: boolean;
	duration: number;
	/** Let clips with root motion travel instead of holding in place. */
	rootMotion: boolean;
}

export class AnimationController {
	readonly entries: readonly ClipEntry[];
	readonly hasRootMotion: boolean;
	private readonly byStem = new Map<string, ClipEntry>();
	private readonly library: ClipLibrary;
	private readonly mixer: AnimationMixer;
	private readonly root: Object3D;
	private action: AnimationAction | null = null;
	private current: { entry: ClipEntry; clip: AnimationClip } | null = null;
	/** In-place variants of root-motion clips, built once per clip. */
	private readonly inPlace = new Map<AnimationClip, AnimationClip>();
	/** Actions fading out; stopped and uncached once their weight hits zero. */
	private retiring: AnimationAction[] = [];
	private selectToken = 0;
	private disposed = false;
	private listeners = new Set<() => void>();
	private snapshot: AnimationState = {
		clip: null,
		loading: null,
		failed: null,
		playing: false,
		speed: 1,
		loop: true,
		duration: 0,
		rootMotion: false,
	};
	/** Set by the canvas so imperative changes trigger a frame. */
	invalidate: () => void = () => {};

	constructor(
		root: Object3D,
		library: ClipLibrary,
		options: { rootMotion?: boolean } = {},
	) {
		this.root = root;
		this.library = library;
		this.entries = library.entries;
		for (const e of this.entries) this.byStem.set(clipStem(e.file), e);
		this.hasRootMotion = this.entries.some((e) => e.rootMotion);
		this.snapshot = {
			...this.snapshot,
			rootMotion: options.rootMotion ?? false,
		};
		this.mixer = new AnimationMixer(root);
		this.mixer.addEventListener("finished", (e) => {
			if (e.action === this.action) this.set({ playing: false });
		});
	}

	get state(): AnimationState {
		return this.snapshot;
	}

	get time(): number {
		return this.action ? this.action.time : 0;
	}

	/** The entry for a `?clip=` value / row id, if the model has it. */
	entry(stem: string): ClipEntry | undefined {
		return this.byStem.get(stem);
	}

	subscribe = (listener: () => void): (() => void) => {
		this.listeners.add(listener);
		return () => this.listeners.delete(listener);
	};

	getSnapshot = (): AnimationState => this.snapshot;

	private set(patch: Partial<AnimationState>): void {
		this.snapshot = { ...this.snapshot, ...patch };
		for (const l of this.listeners) l();
		this.invalidate();
	}

	// ----------------------------------------------------------- selection

	/** Warms the cache for a row the pointer is over. */
	prefetch(stem: string): void {
		const entry = this.byStem.get(stem);
		if (entry) this.library.prefetch(entry);
	}

	/**
	 * Fetches the clip (from cache when possible) and crossfades to it.
	 * Resolves false when the stem is unknown, the load failed, or a later
	 * selection superseded this one.
	 */
	async select(stem: string, autoplay = true): Promise<boolean> {
		const entry = this.byStem.get(stem);
		if (!entry || this.disposed) return false;
		const token = ++this.selectToken;
		const previous = this.current ? clipStem(this.current.entry.file) : null;
		// Optimistic: the row highlights and the transport shows the new
		// length while the file is on its way.
		this.set({
			clip: stem,
			loading: this.library.isReady(entry) ? null : stem,
			failed: null,
			duration: entry.duration,
			loop: entry.loop,
		});
		let clip: AnimationClip;
		try {
			clip = await this.library.load(entry);
		} catch {
			if (token !== this.selectToken || this.disposed) return false;
			this.set({
				clip: previous,
				loading: null,
				failed: stem,
				duration: this.current?.entry.duration ?? 0,
				loop: this.current?.entry.loop ?? true,
			});
			return false;
		}
		if (token !== this.selectToken || this.disposed) return false;
		this.start(entry, clip, autoplay, 0);
		return true;
	}

	/** Builds the action for a clip and swaps it in, fading from the old one. */
	private start(
		entry: ClipEntry,
		clip: AnimationClip,
		autoplay: boolean,
		time: number,
	): void {
		const playable = this.snapshot.rootMotion
			? clip
			: this.inPlaceVariant(entry, clip);
		const next = this.mixer.clipAction(playable);
		const prev = this.action;
		const loop = this.snapshot.loop;
		next.reset();
		next.enabled = true;
		next.setLoop(loop ? LoopRepeat : LoopOnce, Number.POSITIVE_INFINITY);
		next.clampWhenFinished = true;
		next.setEffectiveTimeScale(1);
		next.setEffectiveWeight(1);
		if (prev && prev !== next) {
			if (autoplay) {
				next.play();
				prev.crossFadeTo(next, FADE, false);
				this.retiring.push(prev);
			} else {
				// Paused: nothing would advance the fade, so cut instead.
				prev.stop();
				this.mixer.uncacheAction(prev.getClip(), this.root);
				next.play();
			}
		} else {
			next.play();
		}
		next.time = Math.min(time, playable.duration);
		next.paused = !autoplay;
		this.action = next;
		this.current = { entry, clip };
		this.mixer.update(0);
		this.reap();
		this.set({
			clip: clipStem(entry.file),
			loading: null,
			playing: autoplay,
			duration: playable.duration,
		});
	}

	/** The clip with its root translation pinned to `rootStart`. */
	private inPlaceVariant(entry: ClipEntry, clip: AnimationClip): AnimationClip {
		if (!entry.rootMotion || !entry.rootNode) return clip;
		const cached = this.inPlace.get(clip);
		if (cached) return cached;
		const name = `${entry.rootNode}.position`;
		const index = clip.tracks.findIndex((t) => t.name === name);
		if (index < 0) {
			this.inPlace.set(clip, clip);
			return clip;
		}
		const track = clip.tracks[index];
		const start =
			entry.rootStart ?? (Array.from(track.values.slice(0, 3)) as number[]);
		const tracks = [...clip.tracks];
		tracks[index] = new VectorKeyframeTrack(name, [0], start);
		const variant = new AnimationClip(
			`${clip.name}:inplace`,
			clip.duration,
			tracks,
		);
		this.inPlace.set(clip, variant);
		return variant;
	}

	/** Stops and uncaches faded-out actions. */
	private reap(): void {
		if (!this.retiring.length) return;
		const keep: AnimationAction[] = [];
		for (const a of this.retiring) {
			if (a === this.action) continue;
			// A finished fade disables the action; a clamped pose that is still
			// fading stays enabled with a positive weight.
			if (a.enabled && a.getEffectiveWeight() > 0) {
				keep.push(a);
				continue;
			}
			a.stop();
			this.mixer.uncacheAction(a.getClip(), this.root);
		}
		this.retiring = keep;
	}

	// ----------------------------------------------------------- transport

	play(): void {
		if (!this.action) {
			const first = defaultClip(this.entries);
			if (first) void this.select(clipStem(first.file));
			return;
		}
		if (
			!this.snapshot.loop &&
			this.action.time >= this.snapshot.duration - 1e-3
		) {
			this.action.reset();
			this.action.play();
		}
		this.action.paused = false;
		this.set({ playing: true });
	}

	pause(): void {
		if (this.action) this.action.paused = true;
		this.set({ playing: false });
	}

	toggle(): void {
		if (this.snapshot.playing) this.pause();
		else this.play();
	}

	seek(time: number): void {
		if (!this.action) return;
		const t = Math.max(0, Math.min(time, this.snapshot.duration));
		this.action.time = t;
		this.action.paused = !this.snapshot.playing;
		this.mixer.update(0);
		this.invalidate();
		for (const l of this.listeners) l();
	}

	step(frames: number): void {
		if (!this.action) return;
		this.pause();
		let t = this.action.time + frames * FRAME;
		const d = this.snapshot.duration;
		if (this.snapshot.loop && d > 0) t = ((t % d) + d) % d;
		this.seek(t);
	}

	restart(): void {
		this.seek(0);
		this.play();
	}

	setSpeed(speed: number): void {
		this.set({ speed });
	}

	setLoop(loop: boolean): void {
		this.action?.setLoop(
			loop ? LoopRepeat : LoopOnce,
			Number.POSITIVE_INFINITY,
		);
		this.set({ loop });
	}

	/** Re-plays the current clip in the other root-motion mode, keeping time. */
	setRootMotion(rootMotion: boolean): void {
		if (rootMotion === this.snapshot.rootMotion) return;
		this.set({ rootMotion });
		if (!this.current || !this.current.entry.rootMotion) return;
		const { entry, clip } = this.current;
		this.start(entry, clip, this.snapshot.playing, this.time);
	}

	update(delta: number): void {
		if (!this.snapshot.playing) return;
		this.mixer.update(delta * this.snapshot.speed);
		if (this.retiring.length) this.reap();
	}

	dispose(): void {
		this.disposed = true;
		this.selectToken++;
		this.mixer.stopAllAction();
		this.mixer.uncacheRoot(this.root);
		this.retiring = [];
		this.inPlace.clear();
		this.action = null;
		this.current = null;
		this.listeners.clear();
	}
}
