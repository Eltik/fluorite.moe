/**
 * Thin AnimationMixer wrapper with a subscribable state for the transport UI.
 * The canvas drives `update()` from useFrame while playing; everything else is
 * imperative so the overlay can scrub without re-rendering the scene graph.
 */
import {
	type AnimationAction,
	type AnimationClip,
	AnimationMixer,
	LoopOnce,
	LoopRepeat,
	type Object3D,
} from "three";

export const FRAME = 1 / 30;
export const SPEEDS = [0.25, 0.5, 1, 1.5, 2] as const;

export interface AnimationState {
	clip: string | null;
	playing: boolean;
	speed: number;
	loop: boolean;
	duration: number;
}

export class AnimationController {
	readonly clips: AnimationClip[];
	private readonly mixer: AnimationMixer;
	private readonly root: Object3D;
	private action: AnimationAction | null = null;
	private listeners = new Set<() => void>();
	private snapshot: AnimationState = {
		clip: null,
		playing: false,
		speed: 1,
		loop: true,
		duration: 0,
	};
	/** Set by the canvas so imperative changes trigger a frame. */
	invalidate: () => void = () => {};

	constructor(root: Object3D, clips: AnimationClip[]) {
		this.root = root;
		this.clips = [...clips].sort((a, b) => a.name.localeCompare(b.name));
		this.mixer = new AnimationMixer(root);
		this.mixer.addEventListener("finished", () => {
			this.set({ playing: false });
		});
	}

	get state(): AnimationState {
		return this.snapshot;
	}

	get time(): number {
		return this.action ? this.action.time : 0;
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

	select(name: string, autoplay = true): boolean {
		const clip = this.clips.find((c) => c.name === name);
		if (!clip) return false;
		const next = this.mixer.clipAction(clip);
		next.reset();
		next.setLoop(this.snapshot.loop ? LoopRepeat : LoopOnce, Infinity);
		next.clampWhenFinished = true;
		if (this.action && this.action !== next) {
			next.play();
			this.action.crossFadeTo(next, 0.2, false);
		} else {
			next.play();
		}
		this.action = next;
		next.paused = !autoplay;
		this.mixer.update(0);
		this.set({ clip: clip.name, duration: clip.duration, playing: autoplay });
		return true;
	}

	play(): void {
		if (!this.action) {
			if (this.clips[0]) this.select(this.clips[0].name);
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
		this.action?.setLoop(loop ? LoopRepeat : LoopOnce, Infinity);
		this.set({ loop });
	}

	update(delta: number): void {
		if (this.snapshot.playing) this.mixer.update(delta * this.snapshot.speed);
	}

	dispose(): void {
		this.mixer.stopAllAction();
		this.mixer.uncacheRoot(this.root);
		this.listeners.clear();
	}
}
