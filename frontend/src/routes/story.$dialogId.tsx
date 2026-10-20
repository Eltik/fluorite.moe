import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, notFound } from "@tanstack/react-router";
import {
	BookOpenTextIcon,
	ChevronLeftIcon,
	ChevronRightIcon,
	PlayIcon,
	TypeIcon,
} from "lucide-react";
import * as React from "react";
import { z } from "zod";
import { GenderToggle } from "@/components/catalog/detail-layout";
import { DefaultPending, ErrorState } from "@/components/status-views";
import { StoryReader } from "@/components/story/story-reader";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Toggle } from "@/components/ui/toggle";
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
	const groupLabel = located?.group.title || located?.group.id;

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
							{dialog.title || located?.group.title || dialogId}
						</h1>
						<p className="font-mono text-muted-foreground text-xs">
							{dialogId}
							{siblings.length > 1 && index >= 0 && (
								<span className="font-sans">
									{" "}
									· part {index + 1} of {siblings.length}
								</span>
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
					{mode === "play" && (
						<Toggle
							aria-label="Typewriter reveal"
							onPressedChange={toggleTypewriter}
							pressed={typewriter}
							size="sm"
							title="Reveal lines letter by letter (off when reduced motion is on)"
							variant="outline"
						>
							<TypeIcon />
							<span className="max-sm:hidden">Typewriter</span>
						</Toggle>
					)}
					<div className="flex items-center gap-2">
						<Switch
							checked={Boolean(search.alt)}
							disabled={!hasAlt}
							id="alt-text"
							onCheckedChange={(checked) =>
								setSearch({ alt: checked || undefined })
							}
						/>
						<Label
							className={hasAlt ? "" : "text-muted-foreground"}
							htmlFor="alt-text"
							title={
								hasAlt ? undefined : "This dialog has no gender-specific lines"
							}
						>
							Alternate gender text
						</Label>
					</div>
					<div className="ms-auto flex items-center gap-2">
						<span className="text-muted-foreground text-xs max-sm:hidden">
							Endmin
						</span>
						<GenderToggle
							onChange={(g) => setSearch({ gender: g === "f" ? undefined : g })}
							value={gender}
						/>
					</div>
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
							Part {dialogShortLabel(prev, located?.group.id ?? "")}
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
							Part {dialogShortLabel(next, located?.group.id ?? "")}
							<ChevronRightIcon />
						</Button>
					)}
				</nav>
			)}
		</div>
	);
}
