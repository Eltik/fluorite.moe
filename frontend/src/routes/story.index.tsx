import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import {
	BookOpenIcon,
	ChevronRightIcon,
	HistoryIcon,
	SearchXIcon,
} from "lucide-react";
import * as React from "react";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import {
	isSystemDialog,
	type LastRead,
	loadLastRead,
	operatorIndex,
	type StoryCard,
	type StorySection,
	storySections,
} from "@/components/story/story-labels";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { thumbUrl } from "@/lib/api";
import {
	catalogListQuery,
	loaderLanguage,
	STORY_CATEGORY_LABELS,
	type StoryCategoryId,
	storyListQuery,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { bool, oneOf, searchParams, text } from "@/lib/search-params";
import { useSettings } from "@/lib/settings";

const CATEGORIES = ["mission", "level", "gameplay", "npc"] as const;

const parseSearch = searchParams({
	cat: oneOf(CATEGORIES),
	q: text,
	system: bool,
});

export const Route = createFileRoute("/story/")({
	validateSearch: parseSearch,
	loader: ({ context }) =>
		context.queryClient
			.ensureQueryData(storyListQuery(loaderLanguage()))
			.catch(() => undefined),
	head: () => ({ meta: [{ title: pageTitle("Story") }] }),
	component: StoryIndex,
});

function sectionMatches(
	section: StorySection,
	tokens: string[],
): StorySection | null {
	if (tokens.length === 0) return section;
	const head = `${section.family ?? ""} ${section.title}`.toLowerCase();
	const cards = section.cards.flatMap((card) => {
		const cardHead = `${head} ${card.title} ${card.key}`.toLowerCase();
		if (tokens.every((t) => cardHead.includes(t))) return [card];
		const dialogs = card.dialogs.filter((d) => {
			const hay = `${cardHead} ${d} ${card.labels.get(d) ?? ""}`.toLowerCase();
			return tokens.every((t) => hay.includes(t));
		});
		return dialogs.length ? [{ ...card, dialogs }] : [];
	});
	return cards.length ? { ...section, cards } : null;
}

function StoryIndex(): React.ReactElement {
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const query = useQuery({
		...storyListQuery(textLanguage),
		placeholderData: (prev) => prev,
	});
	const operators = useQuery(catalogListQuery("operators", textLanguage));
	const ops = React.useMemo(
		() => operatorIndex(operators.data?.items),
		[operators.data],
	);
	const cat: StoryCategoryId = search.cat ?? "mission";
	const showSystem = Boolean(search.system);
	const category = query.data?.categories.find((c) => c.id === cat);
	const deferredQ = React.useDeferredValue(search.q ?? "");

	const [lastRead, setLastRead] = React.useState<LastRead | null>(null);
	React.useEffect(() => setLastRead(loadLastRead()), []);

	const sections = React.useMemo(() => {
		if (!category) return [];
		const tokens = deferredQ.trim().toLowerCase().split(/\s+/).filter(Boolean);
		return storySections(cat, category.groups, ops, showSystem)
			.map((s) => sectionMatches(s, tokens))
			.filter((s): s is StorySection => s !== null);
	}, [category, cat, ops, showSystem, deferredQ]);
	const dialogCount = sections.reduce(
		(n, s) => n + s.cards.reduce((m, c) => m + c.dialogs.length, 0),
		0,
	);
	const systemCount = React.useMemo(
		() =>
			category?.groups.reduce(
				(n, g) => n + g.dialogs.filter(isSystemDialog).length,
				0,
			) ?? 0,
		[category],
	);

	return (
		<div className="mx-auto flex w-full max-w-6xl flex-col gap-5 p-4 md:p-6 lg:p-8">
			<PageHeader
				description={
					query.data
						? `${query.data.total.toLocaleString()} dialogs, read as chat transcripts with choices.`
						: "Every dialog, read as a chat transcript with choices."
				}
				icon={<BookOpenIcon />}
				title="Story"
			/>
			{lastRead && !search.q && (
				<Link
					className="group flex items-center gap-3 rounded-xl border bg-card px-4 py-3 shadow-xs/5 outline-none transition-colors hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
					params={{ dialogId: lastRead.id }}
					to="/story/$dialogId"
				>
					<HistoryIcon className="size-4 shrink-0 text-muted-foreground" />
					<span className="min-w-0 flex-1">
						<span className="block text-muted-foreground text-xs">
							Continue where you left off
						</span>
						<span className="block truncate font-medium text-sm">
							{lastRead.title || "Last dialog"}
						</span>
					</span>
					<ChevronRightIcon className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
				</Link>
			)}
			{query.isError && !query.data ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : (
				<>
					<div className="flex flex-wrap items-center justify-between gap-3">
						<Tabs
							className="min-w-0 max-w-full"
							onValueChange={(value) =>
								void navigate({
									search: (prev) => ({
										...prev,
										cat:
											value === "mission"
												? undefined
												: (value as StoryCategoryId),
									}),
									replace: true,
								})
							}
							value={cat}
						>
							<TabsList className="max-w-full justify-start overflow-x-auto [scrollbar-width:none]">
								{CATEGORIES.map((id) => {
									const count = query.data?.categories.find(
										(c) => c.id === id,
									)?.count;
									return (
										<TabsTab key={id} value={id}>
											{STORY_CATEGORY_LABELS[id]}
											{count !== undefined && (
												<span className="text-muted-foreground text-xs tabular-nums">
													{count.toLocaleString()}
												</span>
											)}
										</TabsTab>
									);
								})}
							</TabsList>
						</Tabs>
						<div className="flex flex-wrap items-center gap-3">
							{systemCount > 0 && (
								<div className="flex items-center gap-2">
									<Switch
										checked={showSystem}
										id="story-system"
										onCheckedChange={(checked) =>
											void navigate({
												search: (prev) => ({
													...prev,
													system: checked || undefined,
												}),
												replace: true,
											})
										}
									/>
									<Label
										className="text-muted-foreground text-sm"
										htmlFor="story-system"
										title="UI hooks such as OpenUI / NewSeries that carry no story"
									>
										Show system dialogs
									</Label>
								</div>
							)}
							<SearchInput
								className="w-72 max-w-full"
								label="Search dialogs"
								onCommit={(value) =>
									void navigate({
										search: (prev) => ({ ...prev, q: value || undefined }),
										replace: true,
									})
								}
								placeholder="Search by title, chapter or operator…"
								value={search.q ?? ""}
							/>
						</div>
					</div>
					{!query.data ? (
						<div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
							{Array.from({ length: 9 }, (_, i) => (
								// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
								<Skeleton className="h-28 rounded-xl" key={i} />
							))}
						</div>
					) : sections.length === 0 ? (
						<Empty className="rounded-2xl border border-dashed py-16">
							<EmptyHeader>
								<EmptyMedia variant="icon">
									<SearchXIcon />
								</EmptyMedia>
								<EmptyTitle>No dialogs match</EmptyTitle>
								<EmptyDescription>
									Nothing in {STORY_CATEGORY_LABELS[cat].toLowerCase()} matches
									“{search.q}”.
								</EmptyDescription>
							</EmptyHeader>
						</Empty>
					) : (
						<>
							{search.q && (
								<p className="-mt-2 text-muted-foreground text-sm">
									{dialogCount.toLocaleString()} dialogs in{" "}
									{sections.length.toLocaleString()} sections
								</p>
							)}
							<div className="flex flex-col gap-6">
								{sections.map((section) => (
									<SectionBlock key={section.key} section={section} />
								))}
							</div>
						</>
					)}
				</>
			)}
		</div>
	);
}

function SectionBlock({
	section,
}: {
	section: StorySection;
}): React.ReactElement {
	const headingId = `story-section-${section.key}`;
	return (
		<section
			aria-labelledby={headingId}
			className="[contain-intrinsic-size:auto_20rem] [content-visibility:auto]"
		>
			<header className="sticky top-12 z-10 -mx-2 mb-2 flex items-center gap-2.5 bg-background/88 px-2 py-2 backdrop-blur-md">
				{section.icon && (
					<img
						alt=""
						className="size-7 shrink-0 rounded-full border bg-muted object-cover"
						loading="lazy"
						src={thumbUrl(section.icon, 128)}
					/>
				)}
				<h2
					className="flex min-w-0 items-baseline gap-2 font-heading font-semibold text-base tracking-tight"
					id={headingId}
				>
					{section.family && (
						<span className="shrink-0 font-normal text-muted-foreground text-sm">
							{section.family}
						</span>
					)}
					<span className="truncate">{section.title}</span>
				</h2>
				<span className="ms-auto shrink-0 text-muted-foreground text-xs tabular-nums">
					{section.cards.length === 1
						? `${section.cards[0]?.dialogs.length ?? 0} dialogs`
						: `${section.cards.length} missions`}
				</span>
			</header>
			<ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
				{section.cards.map((card) => (
					<StoryCardView card={card} key={card.key} />
				))}
			</ul>
		</section>
	);
}

function StoryCardView({ card }: { card: StoryCard }): React.ReactElement {
	return (
		<li>
			<div className="flex h-full flex-col gap-2.5 rounded-xl border bg-card p-3.5 shadow-xs/5">
				<div className="flex min-w-0 items-center gap-2">
					{card.icon && (
						<img
							alt=""
							className="size-6 shrink-0 rounded-full border bg-muted object-cover"
							loading="lazy"
							src={thumbUrl(card.icon, 128)}
						/>
					)}
					<span
						className="line-clamp-2 min-w-0 flex-1 font-medium text-sm"
						title={card.title}
					>
						{card.title}
					</span>
					<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
						{card.dialogs.length === 1
							? "1 part"
							: `${card.dialogs.length} parts`}
					</span>
				</div>
				<ul className="flex flex-wrap gap-1.5">
					{card.dialogs.map((dialog) => (
						<li key={dialog}>
							<Link
								className="inline-flex h-7 items-center justify-center rounded-md border bg-background px-2 text-xs outline-none pointer-coarse:h-9 transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring dark:bg-input/32"
								params={{ dialogId: dialog }}
								title={`${card.title} · ${card.labels.get(dialog) ?? dialog}`}
								to="/story/$dialogId"
							>
								{card.labels.get(dialog) ?? dialog}
							</Link>
						</li>
					))}
				</ul>
			</div>
		</li>
	);
}
