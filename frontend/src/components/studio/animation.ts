/**
 * AnimationMixer wrapper with a subscribable state for the transport UI and
 * the animations panel. Clips come from a `ClipLibrary` and are fetched when
 * selected; selection crossfades from whatever is playing. The canvas drives
 * `update()` from useFrame while playing; everything else is imperative so
 * the overlay can scrub without re-rendering the scene graph.
 *
 * With a menu state machine (`lib/menu`), moments play their state and the
 * controller follows the machine's automatic exits (overview start → loop)
 * with the game's crossfade, exit time and offset.
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
import {
	crossed,
	defaultEntry,
	type MenuMachine,
	type MomentPlan,
	nextExit,
	planMoment,
	upcomingClips,
} from "@/lib/menu";

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
	/** The menu moment being played, while the state machine drives clips. */
	moment: string | null;
	/** The menu state currently playing (null for a raw clip). */
	menuState: string | null;
}

export class AnimationController {
	readonly entries: readonly ClipEntry[];
	readonly hasRootMotion: boolean;
	readonly menu: MenuMachine | null;
	private readonly byStem = new Map<string, ClipEntry>();
	private readonly byFile = new Map<string, ClipEntry>();
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
	/** Clip time at the previous tick, for exit detection (-1 = just entered). */
	private prevTime = -1;
	/** A moment waiting for the current clip to reach its `at` time. */
	private pending: (MomentPlan & { moment: string }) | null = null;
	private snapshot: AnimationState = {
		clip: null,
		loading: null,
		failed: null,
		playing: false,
		speed: 1,
		loop: true,
		duration: 0,
		rootMotion: false,
		moment: null,
		menuState: null,
	};
	/** Set by the canvas so imperative changes trigger a frame. */
	invalidate: () => void = () => {};

	constructor(
		root: Object3D,
		library: ClipLibrary,
		options: { rootMotion?: boolean; menu?: MenuMachine | null } = {},
	) {
		this.root = root;
		this.library = library;
		this.entries = library.entries;
		for (const e of this.entries) {
			this.byStem.set(clipStem(e.file), e);
			this.byFile.set(e.file, e);
		}
		this.hasRootMotion = this.entries.some((e) => e.rootMotion);
		this.menu = options.menu ?? null;
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

	/** The entry for a clip file name (as menu states and tracks name them). */
	entryByFile(file: string): ClipEntry | undefined {
		return this.byFile.get(file);
	}

	/** The entry currently playing, if any. */
	get currentEntry(): ClipEntry | null {
		return this.current?.entry ?? null;
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

	/** Warms the cache for the clips a menu state can lead to. */
	private prefetchUpcoming(state: string): void {
		if (!this.menu) return;
		for (const file of upcomingClips(this.menu, state)) {
			const entry = this.byFile.get(file);
			if (entry) this.library.prefetch(entry);
		}
	}

	/**
	 * Fetches the clip (from cache when possible) and crossfades to it.
	 * Resolves false when the stem is unknown, the load failed, or a later
	 * selection superseded this one. Leaves the menu state machine.
	 */
	async select(stem: string, autoplay = true): Promise<boolean> {
		const entry = this.byStem.get(stem);
		if (!entry || this.disposed) return false;
		this.pending = null;
		return this.swapTo(entry, {
			autoplay,
			time: 0,
			fade: FADE,
			loop: entry.loop,
			moment: null,
			menuState: null,
		});
	}

	/**
	 * Plays a menu moment: its state, then whatever the machine exits into.
	 * When the game would wait for the current loop to reach a point first
	 * (`from` + `on`), the moment is queued and starts there.
	 */
	async selectMoment(id: string): Promise<boolean> {
		if (!this.menu || this.disposed) return false;
		const plan = planMoment(this.menu, id, this.snapshot.menuState);
		if (!plan) return false;
		if (plan.at !== null && this.action && this.snapshot.playing) {
			this.pending = { ...plan, moment: id };
			this.set({ moment: id });
			const file = this.menu.states[plan.state]?.clip;
			const entry = file ? this.byFile.get(file) : undefined;
			if (entry) this.library.prefetch(entry);
			return true;
		}
		this.pending = null;
		return this.enterState(plan.state, plan.fade, plan.offset, id);
	}

	/** The moment (or bare state) the machine opens on, if any. */
	playDefaultMoment(): Promise<boolean> {
		if (!this.menu) return Promise.resolve(false);
		const entry = defaultEntry(this.menu);
		if (!entry) return Promise.resolve(false);
		if (entry.moment) return this.selectMoment(entry.moment.id);
		return this.enterState(entry.state, FADE, 0, null);
	}

	private async enterState(
		name: string,
		fade: number,
		offset: number,
		moment: string | null,
	): Promise<boolean> {
		const menu = this.menu;
		const state = menu?.states[name];
		const entry = state?.clip ? this.byFile.get(state.clip) : undefined;
		if (!menu || !state || !entry) return false;
		const ok = await this.swapTo(entry, {
			autoplay: true,
			time: offset,
			fade,
			loop: state.loop,
			moment,
			menuState: name,
		});
		if (ok) this.prefetchUpcoming(name);
		return ok;
	}

	/** Loads an entry and swaps it in; the shared tail of every selection. */
	private async swapTo(
		entry: ClipEntry,
		options: {
			autoplay: boolean;
			time: number;
			fade: number;
			loop: boolean;
			moment: string | null;
			menuState: string | null;
		},
	): Promise<boolean> {
		const stem = clipStem(entry.file);
		const token = ++this.selectToken;
		const previous = this.current ? clipStem(this.current.entry.file) : null;
		// Optimistic: the row highlights and the transport shows the new
		// length while the file is on its way.
		this.set({
			clip: stem,
			loading: this.library.isReady(entry) ? null : stem,
			failed: null,
			duration: entry.duration,
			loop: options.loop,
			moment: options.moment,
			menuState: options.menuState,
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
				moment: null,
				menuState: null,
			});
			return false;
		}
		if (token !== this.selectToken || this.disposed) return false;
		this.start(entry, clip, options.autoplay, options.time, options.fade);
		return true;
	}

	/** Builds the action for a clip and swaps it in, fading from the old one. */
	private start(
		entry: ClipEntry,
		clip: AnimationClip,
		autoplay: boolean,
		time: number,
		fade: number = FADE,
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
				prev.crossFadeTo(next, Math.max(fade, 0.02), false);
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
		this.prevTime = -1;
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
			if (this.menu) {
				void this.playDefaultMoment();
				return;
			}
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
			this.prevTime = -1;
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
		this.prevTime = t;
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
		// Restarting a moment replays it from its transition clip.
		const moment = this.snapshot.moment;
		if (moment && this.menu) {
			void this.selectMoment(moment);
			return;
		}
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
		this.followMenu();
	}

	/** Fires the machine's exit (or a queued moment) once the clip gets there. */
	private followMenu(): void {
		const menu = this.menu;
		const state = this.snapshot.menuState;
		const action = this.action;
		if (!menu || !state || !action || this.snapshot.loading) return;
		const prev = this.prevTime;
		const time = action.time;
		this.prevTime = time;
		const pending = this.pending;
		if (pending) {
			const loop = menu.states[state]?.loop ?? false;
			if (crossed(pending.at ?? 0, prev, time, loop)) {
				this.pending = null;
				void this.enterState(
					pending.state,
					pending.fade,
					pending.offset,
					pending.moment,
				);
			}
			return;
		}
		const exit = nextExit(menu, state, prev, time);
		if (exit) {
			void this.enterState(
				exit.to,
				exit.fade,
				exit.offset,
				this.snapshot.moment,
			);
		}
	}

	dispose(): void {
		this.disposed = true;
		this.selectToken++;
		this.pending = null;
		this.mixer.stopAllAction();
		this.mixer.uncacheRoot(this.root);
		this.retiring = [];
		this.inPlace.clear();
		this.action = null;
		this.current = null;
		this.listeners.clear();
	}
}
