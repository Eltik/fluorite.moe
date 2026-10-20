import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowRightIcon, AudioLinesIcon, BookOpenIcon } from "lucide-react";
import type * as React from "react";
import { AudioButton } from "@/components/audio/audio-button";
import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
	audioSummaryQuery,
	formatDuration,
	type VoiceLanguage,
	voiceQuery,
} from "@/lib/audio";
import { richToPlain, storyListQuery } from "@/lib/catalog";
import { formatNumber } from "@/lib/format";

/** Voice language closest to the text language (EN when there is none). */
function voiceLanguageFor(textLanguage: string): VoiceLanguage {
	if (textLanguage === "CN" || textLanguage === "TC") return "CN";
	if (textLanguage === "JP" || textLanguage === "KR") return textLanguage;
	return "EN";
}

/** Wraps a line in curly quotes unless the game text already starts with one. */
function quoted(text: string): string {
	const t = text.trim();
	return /^["“「『]/.test(t) ? t : `“${t}”`;
}

function EntryCard({
	icon,
	title,
	to,
	cta,
	description,
	children,
}: {
	icon: React.ReactNode;
	title: string;
	to: "/story" | "/audio";
	cta: string;
	description: string;
	children: React.ReactNode;
}): React.ReactElement {
	return (
		<Card className="gap-4 p-5">
			<div className="flex items-start justify-between gap-3">
				<div className="flex items-center gap-3">
					<span className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background text-muted-foreground [&_svg]:size-4.5">
						{icon}
					</span>
					<div>
						<h3 className="font-semibold">{title}</h3>
						<p className="text-muted-foreground text-sm">{description}</p>
					</div>
				</div>
				<Link
					className="flex shrink-0 items-center gap-1 rounded-md font-medium text-sm hover:underline"
					to={to}
				>
					{cta}
					<ArrowRightIcon aria-hidden="true" className="size-4" />
				</Link>
			</div>
			{children}
		</Card>
	);
}

/** A seeded pick from the story list: same on server and client. */
export function StoryCard({
	lang,
	seed,
}: {
	lang: string;
	seed: number;
}): React.ReactElement {
	const story = useQuery({
		...storyListQuery(lang),
		placeholderData: (prev) => prev,
	});
	const missions =
		story.data?.categories
			.find((c) => c.id === "mission")
			?.groups.filter((g) => g.title && g.dialogs.length > 1) ?? [];
	const pick = missions.length ? missions[seed % missions.length] : undefined;
	const first = pick?.dialogs[0];

	return (
		<EntryCard
			cta="All stories"
			description={
				story.data
					? `${formatNumber(story.data.total)} dialogs, read as chat transcripts`
					: "Every dialog, read as a chat transcript"
			}
			icon={<BookOpenIcon />}
			title="Story"
			to="/story"
		>
			{pick && first ? (
				<Link
					className="group flex items-center justify-between gap-3 rounded-xl border bg-muted/40 p-4 transition-colors hover:bg-accent"
					params={{ dialogId: first }}
					to="/story/$dialogId"
				>
					<span className="min-w-0">
						<span className="block text-muted-foreground text-xs">
							Mission · {pick.dialogs.length} parts
						</span>
						<span className="block truncate font-medium">{pick.title}</span>
					</span>
					<span className="flex shrink-0 items-center gap-1 text-muted-foreground text-sm group-hover:text-foreground">
						Start reading
						<ArrowRightIcon aria-hidden="true" className="size-4" />
					</span>
				</Link>
			) : story.isError ? (
				<p className="rounded-xl border bg-muted/40 p-4 text-muted-foreground text-sm">
					Missions, side stories and NPC chatter with every choice intact.
				</p>
			) : (
				<Skeleton className="h-[4.5rem] rounded-xl" />
			)}
		</EntryCard>
	);
}

/** A seeded voice line from the featured operator, playable in place. */
export function AudioCard({
	operatorId,
	lang,
	seed,
}: {
	operatorId: string | undefined;
	lang: string;
	seed: number;
}): React.ReactElement {
	const summary = useQuery(audioSummaryQuery());
	const voice = useQuery({
		...voiceQuery(operatorId ?? "", lang),
		enabled: Boolean(operatorId),
		placeholderData: (prev) => prev,
	});
	const voiceLang = voiceLanguageFor(lang);
	const lines =
		voice.data?.lines.filter((l) => l.audio[voiceLang] && l.text.length) ?? [];
	const line = lines.length ? lines[seed % lines.length] : undefined;
	const voicedLines = summary.data?.coverage.dialogWithAudio;

	return (
		<EntryCard
			cta="Audio library"
			description={
				voicedLines
					? `${formatNumber(voicedLines)} voiced lines in 4 languages`
					: "Voice lines, music and sound effects"
			}
			icon={<AudioLinesIcon />}
			title="Audio"
			to="/audio"
		>
			{line && voice.data ? (
				<figure className="flex items-start gap-3 rounded-xl border bg-muted/40 p-4">
					<AudioButton
						className="mt-0.5 shrink-0"
						label={`Play “${line.title}” by ${voice.data.name}`}
						size="icon"
						src={line.audio[voiceLang]}
						subtitle={`${voice.data.name} · ${voiceLang}`}
						title={line.title}
						variant="outline"
					/>
					<div className="min-w-0">
						<blockquote className="line-clamp-2 text-pretty text-sm leading-relaxed">
							{quoted(richToPlain(line.text))}
						</blockquote>
						<figcaption className="mt-1 text-muted-foreground text-xs">
							{voice.data.name} · {line.title} ·{" "}
							<span className="tabular-nums">
								{formatDuration(line.durations[voiceLang])}
							</span>{" "}
							· {voiceLang} voice
						</figcaption>
					</div>
				</figure>
			) : voice.isError || voice.data || !operatorId ? (
				<p className="rounded-xl border bg-muted/40 p-4 text-muted-foreground text-sm">
					Operator voice lines, combat barks, music and sound effects, decoded
					straight from the game's audio packs.
				</p>
			) : (
				<Skeleton className="h-[4.5rem] rounded-xl" />
			)}
		</EntryCard>
	);
}
