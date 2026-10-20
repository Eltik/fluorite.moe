/**
 * Audio endpoints: Wwise packages indexed by the API and transcoded on demand
 * (`/audio/{pack}/{id}.ogg|m4a`). Clip URLs from the API are root-relative;
 * `audioSrc()` picks the format this browser plays and prefixes the API origin.
 */
import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { API_URL, getJson } from "@/lib/api";
import type { Rich } from "@/lib/catalog";

/** A clip in both formats the API serves (Opus in Ogg, AAC in MP4). */
export interface AudioSources {
	ogg?: string;
	m4a?: string;
}

/** Voice languages first, then sound effects. */
export const VOICE_LANGUAGES = ["CN", "EN", "JP", "KR"] as const;
export type VoiceLanguage = (typeof VOICE_LANGUAGES)[number];

export const AUDIO_LANGUAGE_LABELS: Record<string, string> = {
	CN: "Chinese",
	EN: "English",
	JP: "Japanese",
	KR: "Korean",
	SFX: "SFX",
};

export type AudioKind = "bank" | "stream" | "external";

export interface AudioPack {
	name: string;
	group: string;
	size: number;
	languages: string[];
	counts: Record<AudioKind | "total", number>;
	/** Read from the game CDN by range (voice-language packages). */
	remote: boolean;
}

export interface AudioCoverage {
	streams: number;
	streamsWithSound: number;
	streamsWithEvent: number;
	events: number;
	eventsNamed: number;
	banks: number;
	banksNamed: number;
	externals: number;
	externalsWithDialog: number;
	dialogLines: number;
	dialogWithAudio: number;
}

export interface AudioSummary {
	packs: AudioPack[];
	languages: { code: string; name: string; entries: number }[];
	kinds: Record<AudioKind, number>;
	total: number;
	coverage: AudioCoverage;
	tools: { vgmstream: boolean; ffmpeg: boolean };
	formats: string[];
}

export interface AudioEntry {
	pack: string;
	/** Decimal id; 64-bit for voice lines, so always a string. */
	id: string;
	kind: AudioKind;
	lang: string;
	size: number;
	codec?: string;
	channels?: number;
	duration?: number;
	name?: string;
	voType?: string;
	speaker?: string;
	path?: string;
	events?: { id: string; name?: string }[];
	audio?: AudioSources;
}

export interface AudioEntriesPage {
	total: number;
	offset: number;
	limit: number;
	entries: AudioEntry[];
}

export interface VoiceLine {
	id: string;
	title: string;
	text: Rich;
	voId: string;
	voType?: string;
	durations: Partial<Record<VoiceLanguage, number>>;
	/** Per voice language; empty when the line has no recording. */
	audio: Partial<Record<VoiceLanguage, AudioSources>>;
}

export interface VoiceExtraLine {
	name: string;
	voType: string;
	path: string;
	durations: Partial<Record<VoiceLanguage, number>>;
	audio: Partial<Record<VoiceLanguage, AudioSources>>;
}

export interface VoiceSet {
	charId: string;
	name: string;
	lang: string;
	languages: VoiceLanguage[];
	coverage: { lines: number; withAudio: number };
	lines: VoiceLine[];
	/** Other recordings of the operator (combat barks, interactions, ...). */
	extra: VoiceExtraLine[];
}

export interface AudioFilters {
	pack?: string;
	lang?: string;
	kind?: AudioKind;
	q?: string;
	/** Drop soundbank refs (they have no media to play). */
	playable?: boolean;
}

const PAGE = 200;

export const audioSummaryQuery = () =>
	queryOptions({
		queryKey: ["audio", "summary"],
		queryFn: ({ signal }) =>
			getJson<AudioSummary>("/api/audio", undefined, signal),
		staleTime: 5 * 60_000,
	});

export const audioEntriesQuery = (filters: AudioFilters) =>
	infiniteQueryOptions({
		queryKey: ["audio", "entries", filters],
		queryFn: ({ pageParam, signal }) => {
			const { playable, ...rest } = filters;
			return getJson<AudioEntriesPage>(
				"/api/audio/entries",
				{
					...rest,
					playable: playable ? "true" : undefined,
					offset: pageParam,
					limit: PAGE,
				},
				signal,
			);
		},
		initialPageParam: 0,
		getNextPageParam: (last) => {
			const next = last.offset + last.entries.length;
			return next < last.total && last.entries.length > 0 ? next : undefined;
		},
		staleTime: 5 * 60_000,
		retry: 2,
	});

export const voiceQuery = (charId: string, lang: string) =>
	queryOptions({
		queryKey: ["audio", "voice", charId, lang],
		queryFn: ({ signal }) =>
			getJson<VoiceSet>(
				`/api/voice/${encodeURIComponent(charId)}`,
				{ lang },
				signal,
			),
		staleTime: 5 * 60_000,
	});

let oggSupport: boolean | undefined;

/** Whether this browser decodes Opus in Ogg (Safari before 18.4 does not). */
export function playsOggOpus(): boolean {
	if (oggSupport === undefined) {
		oggSupport =
			typeof document !== "undefined" &&
			document
				.createElement("audio")
				.canPlayType('audio/ogg; codecs="opus"') !== "";
	}
	return oggSupport;
}

/** Absolute URL of the best playable format, or undefined when none. */
export function audioSrc(
	src: AudioSources | string | undefined | null,
): string | undefined {
	if (!src) return undefined;
	const path =
		typeof src === "string"
			? src
			: playsOggOpus()
				? (src.ogg ?? src.m4a)
				: (src.m4a ?? src.ogg);
	if (!path) return undefined;
	return path.startsWith("/") ? `${API_URL}${path}` : path;
}

/** `m:ss` (or `h:mm:ss`) for a duration in seconds. */
export function formatDuration(seconds: number | undefined): string {
	if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0)
		return "–:––";
	const total = Math.floor(seconds);
	const s = total % 60;
	const m = Math.floor(total / 60) % 60;
	const h = Math.floor(total / 3600);
	const ss = String(s).padStart(2, "0");
	return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
