import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import {
	AudioLinesIcon,
	RepeatIcon,
	RotateCcwIcon,
	SearchXIcon,
} from "lucide-react";
import * as React from "react";
import { z } from "zod";
import { audioEngine, useAudioState } from "@/components/audio/engine";
import {
	AudioEntryList,
	entryKey,
	entrySubtitle,
	entryTitle,
} from "@/components/audio/entry-list";
import { PlayerBar } from "@/components/audio/player-bar";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState, ListSkeleton } from "@/components/status-views";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import {
	Select,
	SelectItem,
	SelectPopup,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Toggle } from "@/components/ui/toggle";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
	AUDIO_LANGUAGE_LABELS,
	type AudioEntry,
	type AudioFilters,
	type AudioKind,
	audioEntriesQuery,
	audioSummaryQuery,
} from "@/lib/audio";
import { formatNumber } from "@/lib/format";
import { pageTitle } from "@/lib/route-meta";

const KINDS = ["stream", "external", "bank"] as const;
/** No `kind` in the URL means "playable": everything except soundbank refs. */
type KindFilter = AudioKind | "all" | "playable";
const KIND_OPTIONS: KindFilter[] = [
	"playable",
	"stream",
	"external",
	"bank",
	"all",
];
const KIND_LABELS: Record<KindFilter, string> = {
	playable: "Playable",
	stream: "Sound & music",
	external: "Voice lines",
	bank: "Soundbanks (no media)",
	all: "Everything",
};

/** Skeleton this long means something is wrong; offer a retry. */
const STALL_MS = 6000;

const searchSchema = z.object({
	pack: z.string().optional().catch(undefined),
	lang: z.string().optional().catch(undefined),
	kind: z
		.enum([...KINDS, "all"])
		.optional()
		.catch(undefined),
	q: z.string().optional().catch(undefined),
});

export const Route = createFileRoute("/audio")({
	validateSearch: searchSchema,
	loader: ({ context }) => context.queryClient.query(audioSummaryQuery()),
	head: () => ({ meta: [{ title: pageTitle("Audio") }] }),
	staticData: { section: "Assets", crumb: "Audio" },
	component: AudioPage,
});

/** Keys typed into fields, menus and sliders are theirs, not ours. */
function isTypingTarget(target: EventTarget | null): boolean {
	if (!(target instanceof HTMLElement)) return false;
	return Boolean(
		target.closest(
			"input, textarea, select, [contenteditable=true], [role=combobox], [role=listbox][data-slot], [role=slider], [role=menu]",
		),
	);
}

/** True once `active` has stayed true for `ms` (restarted when `key` changes). */
function useStalled(active: boolean, ms: number, key: string): boolean {
	const [stalled, setStalled] = React.useState(false);
	// biome-ignore lint/correctness/useExhaustiveDependencies: restart per key
	React.useEffect(() => {
		setStalled(false);
		if (!active) return;
		const t = setTimeout(() => setStalled(true), ms);
		return () => clearTimeout(t);
	}, [active, ms, key]);
	return stalled;
}

function AudioPage(): React.ReactElement {
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const setSearch = (patch: Partial<typeof search>) =>
		void navigate({
			search: (prev) => ({ ...prev, ...patch }),
			replace: true,
		});

	const summary = useQuery(audioSummaryQuery());
	const kind = search.kind === "all" ? undefined : search.kind;
	const filters: AudioFilters = {
		pack: search.pack,
		lang: search.lang,
		kind,
		playable: search.kind === undefined ? true : undefined,
		q: search.q?.trim() || undefined,
	};
	const list = useInfiniteQuery(audioEntriesQuery(filters));
	const entries = React.useMemo<AudioEntry[]>(
		() => (list.data?.pages ?? []).flatMap((p) => p.entries),
		[list.data],
	);
	const total = list.data?.pages[0]?.total ?? 0;

	const { hasNextPage, isFetchingNextPage, fetchNextPage } = list;
	const loadMore = React.useCallback(() => {
		if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
	}, [hasNextPage, isFetchingNextPage, fetchNextPage]);

	const playing = useAudioState().track?.key;
	const currentIndex = React.useMemo(
		() => (playing ? entries.findIndex((e) => entryKey(e) === playing) : -1),
		[entries, playing],
	);
	const [selected, setSelected] = React.useState(-1);
	const [autoAdvance, setAutoAdvance] = React.useState(false);

	// A new filter starts the cursor over.
	const filterKey = JSON.stringify(filters);
	const stalled = useStalled(list.isPending, STALL_MS, filterKey);
	// biome-ignore lint/correctness/useExhaustiveDependencies: reset on filter change only
	React.useEffect(() => setSelected(-1), [filterKey]);

	const playAt = React.useCallback(
		(index: number) => {
			const e = entries[index];
			if (!e) return;
			setSelected(index);
			if (!e.audio) {
				audioEngine.unavailable(
					{
						key: entryKey(e),
						src: "",
						title: entryTitle(e),
						subtitle: entrySubtitle(e),
					},
					"Soundbank reference: no decoded media to play",
				);
				return;
			}
			audioEngine.play({
				key: entryKey(e),
				src: e.audio,
				title: entryTitle(e),
				subtitle: entrySubtitle(e),
			});
			if (index >= entries.length - 5) loadMore();
		},
		[entries, loadMore],
	);

	/** Next / previous playable entry from the current clip (or the cursor). */
	const step = React.useCallback(
		(delta: 1 | -1) => {
			let i = (currentIndex >= 0 ? currentIndex : selected) + delta;
			while (i >= 0 && i < entries.length && !entries[i]?.audio) i += delta;
			if (i >= 0 && i < entries.length) playAt(i);
		},
		[currentIndex, selected, entries, playAt],
	);

	React.useEffect(
		() =>
			audioEngine.onEnded(() => {
				if (autoAdvance) step(1);
			}),
		[autoAdvance, step],
	);

	React.useEffect(() => {
		const onKey = (event: KeyboardEvent) => {
			if (event.metaKey || event.ctrlKey || event.altKey) return;
			if (isTypingTarget(event.target)) return;
			const s = audioEngine.getState();
			switch (event.key) {
				case " ":
				case "k":
					if (s.track && s.status !== "error") audioEngine.toggle();
					else {
						// First playable entry at or after the cursor.
						let i = Math.max(0, selected);
						while (i < entries.length && !entries[i]?.audio) i++;
						playAt(i < entries.length ? i : Math.max(0, selected));
					}
					break;
				case "n":
				case "N":
					step(1);
					break;
				case "p":
				case "P":
					step(-1);
					break;
				case "ArrowDown":
				case "j":
					setSelected((i) => Math.min(entries.length - 1, i + 1));
					break;
				case "ArrowUp":
					setSelected((i) => Math.max(0, i - 1));
					break;
				case "Enter":
					if (selected >= 0) playAt(selected);
					else return;
					break;
				case "ArrowRight":
					if (!s.track) return;
					audioEngine.seek(s.time + 5);
					break;
				case "ArrowLeft":
					if (!s.track) return;
					audioEngine.seek(s.time - 5);
					break;
				default:
					return;
			}
			event.preventDefault();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [entries, selected, playAt, step]);

	const data = summary.data;
	const packItems = [
		{ value: "all", label: "All packs" },
		...(data?.packs ?? [])
			.filter((p) =>
				kind
					? p.counts[kind] > 0
					: search.kind === "all" || p.counts.stream + p.counts.external > 0,
			)
			.map((p) => ({
				value: p.name,
				label: `${p.name} (${formatNumber(p.counts.total)})`,
			})),
	];
	const kindItems = KIND_OPTIONS.map((k) => ({
		value: k,
		label: KIND_LABELS[k],
	}));
	const languages = data?.languages.map((l) => l.code) ?? [];
	const voiceLangs = languages.filter((l) => l !== "SFX");
	const toolsMissing = data && (!data.tools.vgmstream || !data.tools.ffmpeg);
	const current = currentIndex >= 0 ? entries[currentIndex] : undefined;

	return (
		<div className="flex min-h-0 w-full flex-1 flex-col gap-3 p-4 md:p-6">
			<PageHeader
				actions={
					<SearchInput
						className="w-72 max-w-full"
						label="Search audio"
						onCommit={(value) => setSearch({ q: value || undefined })}
						placeholder="Name, event, path or id…"
						value={search.q ?? ""}
					/>
				}
				description={
					data
						? `${formatNumber(data.total)} entries in ${data.packs.length} packs${
								voiceLangs.length ? ` · voice in ${voiceLangs.join(", ")}` : ""
							}`
						: "Loading…"
				}
				icon={<AudioLinesIcon />}
				title="Audio"
			/>

			{toolsMissing && (
				<Alert variant="warning">
					<AlertTitle>Playback unavailable</AlertTitle>
					<AlertDescription>
						The API server is missing{" "}
						{!data.tools.vgmstream ? "vgmstream-cli" : "ffmpeg"}, so clips
						cannot be transcoded. Entries can still be browsed.
					</AlertDescription>
				</Alert>
			)}

			<div className="flex flex-wrap items-center gap-2">
				<ToggleGroup
					aria-label="Language"
					onValueChange={(v) => setSearch({ lang: v[0] || undefined })}
					size="sm"
					value={[search.lang ?? ""]}
					variant="outline"
				>
					<ToggleGroupItem value="">All</ToggleGroupItem>
					{languages.map((code) => (
						<ToggleGroupItem
							key={code}
							title={AUDIO_LANGUAGE_LABELS[code] ?? code}
							value={code}
						>
							{code}
						</ToggleGroupItem>
					))}
				</ToggleGroup>
				<Select
					items={kindItems}
					onValueChange={(v) =>
						setSearch({
							kind: v === "playable" ? undefined : (v as AudioKind | "all"),
						})
					}
					value={search.kind ?? "playable"}
				>
					<SelectTrigger
						aria-label="Kind"
						className="w-auto min-w-36"
						size="sm"
					>
						<SelectValue />
					</SelectTrigger>
					<SelectPopup>
						{kindItems.map((item) => (
							<SelectItem key={item.value} value={item.value}>
								{item.label}
							</SelectItem>
						))}
					</SelectPopup>
				</Select>
				<Select
					items={packItems}
					onValueChange={(v) =>
						setSearch({ pack: v === "all" ? undefined : String(v) })
					}
					value={search.pack ?? "all"}
				>
					<SelectTrigger
						aria-label="Pack"
						className="w-auto min-w-44"
						size="sm"
					>
						<SelectValue />
					</SelectTrigger>
					<SelectPopup>
						{packItems.map((item) => (
							<SelectItem key={item.value} value={item.value}>
								{item.label}
							</SelectItem>
						))}
					</SelectPopup>
				</Select>
				<Toggle
					aria-label="Play the next clip when one ends"
					onPressedChange={setAutoAdvance}
					pressed={autoAdvance}
					size="sm"
					variant="outline"
				>
					<RepeatIcon />
					Auto-advance
				</Toggle>
				<span className="ms-auto flex items-center gap-2 text-muted-foreground text-sm tabular-nums">
					{list.isFetching && <Spinner className="size-3.5" />}
					{list.data ? `${formatNumber(total)} matching` : null}
				</span>
			</div>

			<div className="flex min-h-64 flex-1 flex-col overflow-hidden rounded-xl border bg-card shadow-xs/5">
				{list.isPending ? (
					stalled ? (
						<Empty role="status">
							<EmptyHeader>
								<EmptyMedia variant="icon">
									<Spinner />
								</EmptyMedia>
								<EmptyTitle>Still loading…</EmptyTitle>
								<EmptyDescription>
									The audio index is taking longer than usual. The API may still
									be starting up, or the connection stalled.
								</EmptyDescription>
							</EmptyHeader>
							<EmptyContent>
								<Button
									onClick={() => void list.refetch()}
									size="sm"
									variant="outline"
								>
									<RotateCcwIcon />
									Retry
								</Button>
							</EmptyContent>
						</Empty>
					) : (
						<ListSkeleton rows={14} />
					)
				) : list.isError ? (
					<ErrorState error={list.error} onRetry={() => void list.refetch()} />
				) : entries.length === 0 ? (
					<Empty>
						<EmptyHeader>
							<EmptyMedia variant="icon">
								<SearchXIcon />
							</EmptyMedia>
							<EmptyTitle>No audio matches</EmptyTitle>
							<EmptyDescription>
								Try another language, pack or search term.
							</EmptyDescription>
						</EmptyHeader>
					</Empty>
				) : (
					<AudioEntryList
						className="flex-1"
						entries={entries}
						onNearEnd={loadMore}
						onSelect={(index, play) =>
							play ? playAt(index) : setSelected(index)
						}
						selected={selected}
						total={total}
					/>
				)}
			</div>

			<PlayerBar
				emptyHint="Pick a clip, or press Space to play the first one"
				fallbackDuration={current?.duration}
				onNext={entries.length ? () => step(1) : undefined}
				onPrevious={entries.length ? () => step(-1) : undefined}
			/>
		</div>
	);
}
