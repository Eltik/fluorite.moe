/**
 * The character-screen state machine (`clips/<kind>/<id>/menu.json`): a
 * flattened Unity AnimatorController. Pure functions only, so the Studio's
 * three.js glue stays thin and this can be unit tested.
 *
 * A *moment* is a menu entry (Overview, Weapon, Team Pose 1…) that plays a
 * state; most states exit automatically into a loop (`exit`) once their clip
 * reaches `at` seconds, crossfading over `fade` seconds and starting the next
 * clip at `offset` seconds. `on` transitions are parameter-driven in the game
 * and are only honoured where a moment declares the state it comes `from`.
 */

export interface MenuTransition {
	to: string;
	/** Clip time (seconds) at which the transition fires; null = immediately. */
	at: number | null;
	/** Crossfade length in seconds. */
	fade: number;
	/** Start time of the target clip, in seconds. */
	offset: number;
}

export interface MenuState {
	name: string;
	/** Clip file inside the clip folder, or null when it was not exported. */
	clip: string | null;
	length: number;
	loop: boolean;
	exit: MenuTransition | null;
	on: MenuTransition[];
	/** Set by the exporter when the game clip is missing from the export. */
	missingClip: boolean;
}

export interface MenuMoment {
	id: string;
	label: string;
	/** The state the moment plays (usually a transition into `idle`). */
	state: string;
	/** The loop the moment settles in. */
	idle: string;
	/** The state the game plays this moment from (e.g. the relax idle). */
	from?: string;
}

export interface MenuMachine {
	id: string;
	default: string;
	states: Record<string, MenuState>;
	moments: MenuMoment[];
}

/** Crossfade for moments the game would jump into through an AnyState rule. */
export const MOMENT_FADE = 0.25;

function num(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value)
		? value
		: undefined;
}

function str(value: unknown): string | undefined {
	return typeof value === "string" && value ? value : undefined;
}

function transition(value: unknown): MenuTransition | null {
	if (!value || typeof value !== "object") return null;
	const r = value as Record<string, unknown>;
	const to = str(r.to);
	if (!to) return null;
	return {
		to,
		at: num(r.at) ?? null,
		fade: Math.max(0, num(r.fade) ?? num(r.fadeOfSource) ?? 0),
		offset: Math.max(0, num(r.offset) ?? 0),
	};
}

/** Validates a `menu.json` payload; null when it is unusable. */
export function parseMenu(json: unknown): MenuMachine | null {
	if (!json || typeof json !== "object") return null;
	const r = json as Record<string, unknown>;
	if (!r.states || typeof r.states !== "object") return null;
	const states: Record<string, MenuState> = {};
	for (const [name, raw] of Object.entries(
		r.states as Record<string, unknown>,
	)) {
		if (!raw || typeof raw !== "object") continue;
		const s = raw as Record<string, unknown>;
		const on = Array.isArray(s.on)
			? s.on.map(transition).filter((t): t is MenuTransition => t !== null)
			: [];
		states[name] = {
			name,
			clip: str(s.clip) ?? null,
			length: Math.max(0, num(s.length) ?? 0),
			loop: s.loop === true,
			exit: transition(s.exit),
			on,
			missingClip: s.missingClip === true,
		};
	}
	if (!Object.keys(states).length) return null;
	const moments: MenuMoment[] = [];
	const seen = new Set<string>();
	if (Array.isArray(r.moments)) {
		for (const raw of r.moments) {
			if (!raw || typeof raw !== "object") continue;
			const m = raw as Record<string, unknown>;
			const id = str(m.id);
			const state = str(m.state);
			if (!id || !state || seen.has(id)) continue;
			seen.add(id);
			const from = str(m.from);
			moments.push({
				id,
				label: str(m.label) ?? id,
				state,
				idle: str(m.idle) ?? state,
				...(from ? { from } : {}),
			});
		}
	}
	const fallbackDefault =
		Object.values(states).find((s) => s.loop && s.clip)?.name ??
		Object.keys(states)[0];
	return {
		id: str(r.id) ?? "",
		default: str(r.default) ?? fallbackDefault,
		states,
		moments,
	};
}

/** Whether a state can be played: it exists and its clip was exported. */
export function stateAvailable(
	machine: MenuMachine,
	name: string | undefined,
): boolean {
	if (!name) return false;
	const s = machine.states[name];
	return Boolean(s?.clip && !s.missingClip);
}

/** A moment is available when the state it plays is. */
export function momentAvailable(
	machine: MenuMachine,
	moment: MenuMoment,
): boolean {
	return stateAvailable(machine, moment.state);
}

/**
 * What to play when the model opens: the moment that settles into the
 * machine's default loop (the overview start, then its loop), else the first
 * available moment, else the default loop itself.
 */
export function defaultEntry(
	machine: MenuMachine,
): { moment: MenuMoment | null; state: string } | null {
	const byIdle = machine.moments.find(
		(m) => m.idle === machine.default && momentAvailable(machine, m),
	);
	if (byIdle) return { moment: byIdle, state: byIdle.state };
	const first = machine.moments.find((m) => momentAvailable(machine, m));
	if (first) return { moment: first, state: first.state };
	if (stateAvailable(machine, machine.default)) {
		return { moment: null, state: machine.default };
	}
	return null;
}

export interface MomentPlan {
	state: string;
	fade: number;
	offset: number;
	/**
	 * When set, the current clip keeps playing until it reaches this time,
	 * then the transition fires (the game's `on` rule from the `from` state).
	 */
	at: number | null;
}

/**
 * How to play a moment from the current state. Honours the game's timed
 * transition when the moment declares a `from` state and we are in it;
 * otherwise the moment starts right away behind a short crossfade.
 */
export function planMoment(
	machine: MenuMachine,
	momentId: string,
	currentState: string | null,
): MomentPlan | null {
	const moment = machine.moments.find((m) => m.id === momentId);
	if (!moment || !momentAvailable(machine, moment)) return null;
	if (moment.from && currentState === moment.from) {
		const rule = machine.states[moment.from]?.on.find(
			(t) => t.to === moment.state,
		);
		if (rule) {
			return {
				state: moment.state,
				fade: rule.fade,
				offset: rule.offset,
				at: rule.at,
			};
		}
	}
	return { state: moment.state, fade: MOMENT_FADE, offset: 0, at: null };
}

/**
 * Whether a clip playing `state` crossed `at` between two samples of its
 * time. Looping clips wrap, so a wrap counts when `at` lies after the
 * previous sample or before the new one. A negative `prevTime` means the
 * state was just entered, so any time at or past `at` fires.
 */
export function crossed(
	at: number,
	prevTime: number,
	time: number,
	loop: boolean,
): boolean {
	if (prevTime < 0) return time >= at;
	if (loop && time < prevTime) return at > prevTime || at <= time;
	return prevTime < at && time >= at;
}

/**
 * The automatic exit to follow after the clip of `state` advanced from
 * `prevTime` to `time`, or null to keep playing. Exits into states without
 * an exported clip are skipped (the current loop just continues).
 */
export function nextExit(
	machine: MenuMachine,
	state: string,
	prevTime: number,
	time: number,
): MenuTransition | null {
	const s = machine.states[state];
	const exit = s?.exit;
	if (!s || !exit) return null;
	if (!stateAvailable(machine, exit.to)) return null;
	const at = exit.at ?? 0;
	return crossed(at, prevTime, time, s.loop) ? exit : null;
}

/** The exported clip file of a state, if any. */
export function stateClip(machine: MenuMachine, name: string): string | null {
	const s = machine.states[name];
	return s && !s.missingClip ? s.clip : null;
}

/** The clips a state can lead to next, for prefetching. */
export function upcomingClips(machine: MenuMachine, state: string): string[] {
	const s = machine.states[state];
	if (!s) return [];
	const out: string[] = [];
	for (const t of [s.exit, ...s.on]) {
		if (!t) continue;
		const clip = stateClip(machine, t.to);
		if (clip && !out.includes(clip)) out.push(clip);
	}
	return out;
}
