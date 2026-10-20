import { RepeatIcon, SearchIcon } from "lucide-react";
import * as React from "react";
import { AudioButton } from "@/components/audio/audio-button";
import { audioEngine, useAudioState } from "@/components/audio/engine";
import { PlayerBar } from "@/components/audio/player-bar";
import { RichText } from "@/components/rich-text/rich-text";
import {
	InputGroup,
	InputGroupAddon,
	InputGroupInput,
} from "@/components/ui/input-group";
import { Toggle } from "@/components/ui/toggle";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
	AUDIO_LANGUAGE_LABELS,
	type AudioSources,
	VOICE_LANGUAGES,
	type VoiceLanguage,
} from "@/lib/audio";
import { type OperatorDetail as Operator, richToPlain } from "@/lib/catalog";
import { useSettings } from "@/lib/settings";
import { cn } from "@/lib/utils";

type Line = Operator["voiceLines"][number];

/** Track key of a clip; matches `AudioButton`'s so rows know they are current. */
function clipKey(src: AudioSources | undefined): string | undefined {
	return src ? (src.ogg ?? src.m4a) : undefined;
}

/** The voice language matching the text language, when the game records one. */
function defaultVoiceLanguage(textLanguage: string): VoiceLanguage {
	const lang = textLanguage === "TC" ? "CN" : textLanguage;
	return (VOICE_LANGUAGES as readonly string[]).includes(lang)
		? (lang as VoiceLanguage)
		: "EN";
}

/** Thin progress underline for the row that is playing. */
function RowProgress(): React.ReactElement {
	const s = useAudioState();
	const pct = s.duration > 0 ? Math.min(100, (s.time / s.duration) * 100) : 0;
	return (
		<span
			aria-hidden
			className="absolute inset-x-0 bottom-0 h-0.5 bg-primary/15"
		>
			<span
				className="block h-full bg-primary transition-[width] duration-200 ease-linear"
				style={{ width: `${pct}%` }}
			/>
		</span>
	);
}

/**
 * Voice-line list for an operator: per-line play buttons in the chosen voice
 * language, the playing row highlighted with a progress underline, a sticky
 * mini player while one of these lines is current, and optional auto-play of
 * the next line.
 */
export function VoiceLines({ data }: { data: Operator }): React.ReactElement {
	const [filter, setFilter] = React.useState("");
	const { textLanguage } = useSettings();
	const [voiceLang, setVoiceLang] = React.useState<VoiceLanguage>(() =>
		defaultVoiceLanguage(textLanguage),
	);
	const [autoNext, setAutoNext] = React.useState(true);
	const hasAudio = data.voiceLines.some((l) => l.audio);
	const needle = filter.trim().toLowerCase();
	const lines = needle
		? data.voiceLines.filter(
				(l) =>
					l.title.toLowerCase().includes(needle) ||
					richToPlain(l.text).toLowerCase().includes(needle),
			)
		: data.voiceLines;

	const audio = useAudioState();
	const currentKey = audio.track?.key;
	/** Index (in `lines`) of the line that is current, in any voice language. */
	const currentIndex = currentKey
		? lines.findIndex((l) =>
				VOICE_LANGUAGES.some((lang) => clipKey(l.audio?.[lang]) === currentKey),
			)
		: -1;

	const playLine = React.useCallback(
		(line: Line, lang: VoiceLanguage) => {
			const src = line.audio?.[lang];
			const key = clipKey(src);
			if (!src || !key) return;
			audioEngine.play({
				key,
				src,
				title: line.title,
				subtitle: `${data.name} · ${AUDIO_LANGUAGE_LABELS[lang] ?? lang}`,
			});
		},
		[data.name],
	);

	/** Next / previous line with a recording in the current voice language. */
	const step = React.useCallback(
		(delta: 1 | -1) => {
			let i = currentIndex + delta;
			while (i >= 0 && i < lines.length && !lines[i]?.audio?.[voiceLang])
				i += delta;
			const line = lines[i];
			if (line) playLine(line, voiceLang);
		},
		[currentIndex, lines, voiceLang, playLine],
	);

	React.useEffect(
		() =>
			audioEngine.onEnded(() => {
				if (autoNext && currentIndex >= 0) step(1);
			}),
		[autoNext, currentIndex, step],
	);

	const changeVoice = (lang: VoiceLanguage) => {
		setVoiceLang(lang);
		// Keep listening to the same line, now in the new language.
		const line = lines[currentIndex];
		const playing = audio.status === "playing" || audio.status === "loading";
		if (line && playing) playLine(line, lang);
	};

	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-wrap items-center gap-3">
				<InputGroup className="max-w-sm">
					<InputGroupInput
						aria-label="Filter voice lines"
						onChange={(e) => setFilter(e.target.value)}
						placeholder="Filter voice lines…"
						size="sm"
						type="search"
						value={filter}
					/>
					<InputGroupAddon>
						<SearchIcon />
					</InputGroupAddon>
				</InputGroup>
				{hasAudio && (
					<div className="flex flex-wrap items-center gap-2">
						<span
							className="text-muted-foreground text-xs"
							id="voice-language-label"
						>
							Voice
						</span>
						<ToggleGroup
							aria-labelledby="voice-language-label"
							onValueChange={(next) => {
								const v = next[0];
								if (v) changeVoice(v as VoiceLanguage);
							}}
							size="sm"
							value={[voiceLang]}
							variant="outline"
						>
							{VOICE_LANGUAGES.map((lang) => (
								<ToggleGroupItem
									className="data-pressed:bg-primary data-pressed:text-primary-foreground dark:data-pressed:bg-primary"
									key={lang}
									title={`${AUDIO_LANGUAGE_LABELS[lang] ?? lang} voice (text stays in ${textLanguage})`}
									value={lang}
								>
									{lang}
								</ToggleGroupItem>
							))}
						</ToggleGroup>
						<Toggle
							aria-label="Play the next line when one ends"
							onPressedChange={setAutoNext}
							pressed={autoNext}
							size="sm"
							title="Play the next line when one ends"
							variant="outline"
						>
							<RepeatIcon />
							Auto-play
						</Toggle>
					</div>
				)}
			</div>
			<ul className="divide-y overflow-hidden rounded-xl border bg-card">
				{lines.map((line, index) => {
					const active = index === currentIndex;
					const src = line.audio?.[voiceLang];
					return (
						<li
							aria-current={active ? "true" : undefined}
							className={cn(
								"relative flex gap-4 px-4 py-3 transition-colors max-sm:flex-col max-sm:gap-1",
								active &&
									"bg-primary/6 shadow-[inset_2px_0_0_var(--color-primary)]",
							)}
							key={line.id}
						>
							<div className="flex shrink-0 items-start gap-2 sm:w-44">
								<AudioButton
									src={src}
									subtitle={`${data.name} · ${AUDIO_LANGUAGE_LABELS[voiceLang] ?? voiceLang}`}
									title={line.title}
								/>
								<span
									className={cn(
										"font-medium text-xs leading-5",
										active ? "text-primary" : "text-muted-foreground",
									)}
								>
									{line.title}
								</span>
							</div>
							<RichText
								as="p"
								className="min-w-0 max-w-prose flex-1 text-pretty text-sm leading-relaxed"
								value={line.text}
							/>
							{active && <RowProgress />}
						</li>
					);
				})}
				{lines.length === 0 && (
					<li className="px-4 py-10 text-center text-muted-foreground text-sm">
						No voice lines match “{filter}”.
					</li>
				)}
			</ul>
			{currentIndex >= 0 && (
				<PlayerBar
					className="sticky bottom-3 z-10 shadow-lg/10 backdrop-blur supports-[backdrop-filter]:bg-card/85"
					shortcuts={false}
					emptyHint="Nothing playing"
					onNext={() => step(1)}
					onPrevious={() => step(-1)}
				/>
			)}
		</div>
	);
}
