import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, notFound } from "@tanstack/react-router";
import {
	BookOpenTextIcon,
	ChevronLeftIcon,
	ChevronRightIcon,
	PlayIcon,
	Settings2Icon,
} from "lucide-react";
import * as React from "react";
import { z } from "zod";
import { GenderToggle } from "@/components/catalog/detail-layout";
import { DefaultPending, ErrorState } from "@/components/status-views";
import {
	cleanSpeakerName,
	partLabel,
	saveLastRead,
} from "@/components/story/story-labels";
import { StoryReader } from "@/components/story/story-reader";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Label } from "@/components/ui/label";
import {
	Popover,
	PopoverPopup,
	PopoverTitle,
	PopoverTrigger,
} from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { isApiError } from "@/lib/api";
import {
	dialogQuery,
	dialogShortLabel,
	type Gender,
	loaderLanguage,
	locateDialog,
	STORY_CATEGORY_LABELS,
	searchText,
	storyListQuery,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

const searchSchema = z.object({
	mode: z.enum(["play", "all"]).optional().catch(undefined),
	// Node ids are numeric: the router parses `?line=17` as a number.
	line: searchText,
	alt: z.boolean().optional().catch(undefined),
	gender: z.enum(["f", "m"]).optional().catch(undefined),
});

const TYPEWRITER_KEY = "fluorite.story.typewriter";

function readTypewriter(): boolean {
	try {
		return window.localStorage.getItem(TYPEWRITER_KEY) !== "off";
	} catch {
		return true;
	}
}

export const Route = createFileRoute("/story/$dialogId")({
	validateSearch: searchSchema,
	loaderDeps: ({ search }) => ({ gender: (search.gender ?? "f") as Gender }),
	loader: async ({ context, params, deps }) => {
		const lang = loaderLanguage();
		void context.queryClient.prefetchQuery(storyListQuery(lang));
		try {
			const dialog = await context.queryClient.ensureQueryData(
				dialogQuery(params.dialogId, lang, deps.gender),
			);
			return { title: dialog.title ?? undefined };
		} catch (error) {
			if (isApiError(error) && error.isNotFound) throw notFound();
			throw error;
		}
	},
	head: ({ params, loaderData }) => ({
		meta: [
			{
				title: pageTitle(
					loaderData?.title
						? `${loaderData.title} (${params.dialogId})`
						: params.dialogId,
					"Story",
				),
			},
		],
	}),
	staticData: {
		crumb: ({ dialogId }, loaderData) =>
			(loaderData as { title?: string } | undefined)?.title ?? dialogId ?? "",
	},
	component: DialogPage,
});

function DialogPage(): React.ReactElement {
	const { dialogId } = Route.useParams();
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const gender: Gender = search.gender ?? "f";
	const mode = search.mode ?? "play";
	const query = useQuery({
		...dialogQuery(dialogId, textLanguage, gender),
		placeholderData: (prev, prevQuery) =>
			prevQuery?.queryKey[3] === dialogId ? prev : undefined,
	});
	const list = useQuery(storyListQuery(textLanguage));
	const located = locateDialog(list.data, dialogId);

	const loadedTitle = query.data?.title;
	const loadedId = query.data?.id;
	React.useEffect(() => {
		if (!loadedId) return;
		const g = located?.group;
		const label = g ? partLabel(dialogShortLabel(loadedId, g.id)) : undefined;
		const title = loadedTitle || g?.title;
		saveLastRead({
			id: loadedId,
			title: title && label ? `${title} · ${label}` : (title ?? label),
			at: Date.now(),
		});
	}, [loadedId, loadedTitle, located?.group]);

	const [typewriter, setTypewriter] = React.useState(true);
	React.useEffect(() => setTypewriter(readTypewriter()), []);
	const toggleTypewriter = (on: boolean) => {
		setTypewriter(on);
		try {
			window.localStorage.setItem(TYPEWRITER_KEY, on ? "on" : "off");
		} catch {
			// Storage unavailable: keep the in-memory choice.
		}
	};

	const setSearch = React.useCallback(
		(patch: Partial<z.infer<typeof searchSchema>>) =>
			void navigate({
				search: (prev) => ({ ...prev, ...patch }),
				replace: true,
				resetScroll: false,
			}),
		[navigate],
	);
	const onLineChange = React.useCallback(
		(line: string | undefined) => setSearch({ line }),
		[setSearch],
	);

	if (!query.data) {
		if (query.isError) {
			return (
				<ErrorState
					className="min-h-[60vh]"
					error={query.error}
					onRetry={() => void query.refetch()}
				/>
			);
		}
		return <DefaultPending />;
	}
	const dialog = query.data;
	const hasAlt = dialog.nodes.some((n) => n.altText);
	const siblings = located?.group.dialogs ?? [];
	const index = located?.index ?? -1;
	const prev = index > 0 ? siblings[index - 1] : undefined;
	const next =
		index >= 0 && index < siblings.length - 1 ? siblings[index + 1] : undefined;
	const groupLabel = located?.group.title || undefined;
	const groupId = located?.group.id ?? dialog.group ?? "";
	const part = (id: string) => partLabel(dialogShortLabel(id, groupId));
	const heading = dialog.title || located?.group.title || part(dialogId);
	const speakers = [
		...new Set(
			dialog.nodes
				.map((n) => n.speaker?.name)
				.filter((n): n is string => Boolean(n))
				.map(cleanSpeakerName),
		),
	];

	return (
		<div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 md:p-6 lg:p-8">
			<header className="flex flex-col gap-3">
				<nav aria-label="Breadcrumb" className="text-muted-foreground text-xs">
					<Link className="hover:text-foreground" to="/story">
						Story
					</Link>
					{located && (
						<>
							{" › "}
							<Link
								className="hover:text-foreground"
								search={{
									cat:
										located.category.id === "mission"
											? undefined
											: located.category.id,
									q: located.group.id || undefined,
								}}
								to="/story"
							>
								{STORY_CATEGORY_LABELS[located.category.id]}
								{groupLabel ? ` · ${groupLabel}` : ""}
							</Link>
						</>
					)}
				</nav>
				<div className="flex flex-wrap items-end justify-between gap-3">
					<div className="min-w-0">
						<h1 className="text-balance font-heading font-semibold text-xl tracking-tight sm:text-2xl">
							{heading}
						</h1>
						<p className="text-muted-foreground text-xs" title={dialogId}>
							{siblings.length > 1 && index >= 0
								? `${part(dialogId)} · ${index + 1} of ${siblings.length}`
								: part(dialogId)}
							{speakers.length > 0 && (
								<>
									{" · "}
									{speakers.slice(0, 4).join(", ")}
									{speakers.length > 4 ? ` +${speakers.length - 4}` : ""}
								</>
							)}
						</p>
					</div>
				</div>
				<div className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border bg-card px-3 py-2 shadow-xs/5">
					<ToggleGroup
						aria-label="Reading mode"
						onValueChange={(v) => {
							const m = v[0];
							if (m === "play" || m === "all")
								setSearch({ mode: m === "play" ? undefined : m });
						}}
						size="sm"
						value={[mode]}
						variant="outline"
					>
						<ToggleGroupItem value="play">
							<PlayIcon />
							Play
						</ToggleGroupItem>
						<ToggleGroupItem value="all">
							<BookOpenTextIcon />
							Read all
						</ToggleGroupItem>
					</ToggleGroup>
					<Popover>
						<PopoverTrigger
							render={
								<Button
									aria-label="Reader settings"
									size="sm"
									title="Reader settings"
									variant="outline"
								/>
							}
						>
							<Settings2Icon />
							<span className="max-sm:hidden">Settings</span>
						</PopoverTrigger>
						<PopoverPopup align="start" className="w-72">
							<PopoverTitle className="mb-3 text-sm">Reader</PopoverTitle>
							<div className="flex flex-col gap-3">
								<div className="flex items-start gap-3">
									<Switch
										aria-describedby="typewriter-hint"
										checked={typewriter}
										disabled={mode !== "play"}
										id="typewriter"
										onCheckedChange={toggleTypewriter}
									/>
									<div className="flex flex-col gap-0.5">
										<Label htmlFor="typewriter">Typewriter reveal</Label>
										<span
											className="text-muted-foreground text-xs"
											id="typewriter-hint"
										>
											Play mode types lines out letter by letter (off when
											reduced motion is on).
										</span>
									</div>
								</div>
								<div className="flex items-start gap-3">
									<Switch
										aria-describedby="alt-text-hint"
										aria-label="Alternate gender text"
										checked={Boolean(search.alt)}
										disabled={!hasAlt}
										id="alt-text"
										onCheckedChange={(checked) =>
											setSearch({ alt: checked || undefined })
										}
									/>
									<div className="flex flex-col gap-0.5">
										<Label
											className={hasAlt ? "" : "text-muted-foreground"}
											htmlFor="alt-text"
										>
											Show alternate gender text
										</Label>
										<span
											className="text-muted-foreground text-xs"
											id="alt-text-hint"
										>
											{hasAlt
												? "Also show how gendered lines read for the other Endministrator."
												: "This dialog has no gender-specific lines."}
										</span>
									</div>
								</div>
							</div>
						</PopoverPopup>
					</Popover>
					<fieldset
						className="ms-auto flex min-w-0 items-center gap-2 border-0 p-0"
						title="Which Endministrator the dialog addresses ({F}/{M} text)"
					>
						<legend className="sr-only">Endministrator variant</legend>
						<span
							aria-hidden
							className="text-muted-foreground text-xs max-sm:hidden"
						>
							Endministrator
						</span>
						<GenderToggle
							onChange={(g) => setSearch({ gender: g === "f" ? undefined : g })}
							value={gender}
						/>
					</fieldset>
				</div>
				{mode === "play" && (
					<p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs max-sm:hidden">
						<span>
							<Kbd>Space</Kbd> / <Kbd>Enter</Kbd> next
						</span>
						<span>
							<Kbd>←</Kbd> back
						</span>
						<span>
							<Kbd>1</Kbd>–<Kbd>9</Kbd> choose
						</span>
					</p>
				)}
			</header>

			<section aria-label="Dialog transcript" className="min-h-[50vh]">
				<StoryReader
					dialog={dialog}
					key={`${dialog.id}:${dialog.gender}:${dialog.lang}:${mode}`}
					line={search.line}
					mode={mode}
					onLineChange={onLineChange}
					showAlt={Boolean(search.alt)}
					typewriter={typewriter}
				/>
			</section>

			{(prev || next) && (
				<nav
					aria-label="Dialogs in this group"
					className="flex items-center justify-between gap-3 border-t pt-4"
				>
					{prev ? (
						<Button
							render={
								<Link
									params={{ dialogId: prev }}
									search={{}}
									to="/story/$dialogId"
								/>
							}
							size="sm"
							variant="outline"
						>
							<ChevronLeftIcon />
							{part(prev)}
						</Button>
					) : (
						<span />
					)}
					{next && (
						<Button
							render={
								<Link
									params={{ dialogId: next }}
									search={{}}
									to="/story/$dialogId"
								/>
							}
							size="sm"
							variant="outline"
						>
							{part(next)}
							<ChevronRightIcon />
						</Button>
					)}
				</nav>
			)}
		</div>
	);
}
