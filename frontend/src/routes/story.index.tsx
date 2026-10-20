import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { BookOpenIcon, SearchXIcon } from "lucide-react";
import * as React from "react";
import { z } from "zod";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import {
	dialogShortLabel,
	loaderLanguage,
	STORY_CATEGORY_LABELS,
	type StoryCategoryId,
	type StoryGroup,
	searchText,
	storyListQuery,
} from "@/lib/catalog";
import { pageTitle } from "@/lib/route-meta";
import { useSettings } from "@/lib/settings";

const CATEGORIES = ["mission", "level", "gameplay", "npc"] as const;

const searchSchema = z.object({
	cat: z.enum(CATEGORIES).optional().catch(undefined),
	q: searchText,
});

export const Route = createFileRoute("/story/")({
	validateSearch: searchSchema,
	loader: ({ context }) =>
		context.queryClient
			.ensureQueryData(storyListQuery(loaderLanguage()))
			.catch(() => undefined),
	head: () => ({ meta: [{ title: pageTitle("Story") }] }),
	component: StoryIndex,
});

function matches(group: StoryGroup, tokens: string[]): StoryGroup | null {
	if (tokens.length === 0) return group;
	const head = `${group.title ?? ""} ${group.id}`.toLowerCase();
	if (tokens.every((t) => head.includes(t))) return group;
	const dialogs = group.dialogs.filter((d) => {
		const hay = d.toLowerCase();
		return tokens.every((t) => hay.includes(t) || head.includes(t));
	});
	return dialogs.length ? { ...group, dialogs } : null;
}

function StoryIndex(): React.ReactElement {
	const search = Route.useSearch();
	const navigate = Route.useNavigate();
	const { textLanguage } = useSettings();
	const query = useQuery({
		...storyListQuery(textLanguage),
		placeholderData: (prev) => prev,
	});
	const cat: StoryCategoryId = search.cat ?? "mission";
	const category = query.data?.categories.find((c) => c.id === cat);
	const deferredQ = React.useDeferredValue(search.q ?? "");

	const groups = React.useMemo(() => {
		if (!category) return [];
		const tokens = deferredQ.trim().toLowerCase().split(/\s+/).filter(Boolean);
		return category.groups
			.map((g) => matches(g, tokens))
			.filter((g): g is StoryGroup => g !== null);
	}, [category, deferredQ]);
	const dialogCount = groups.reduce((n, g) => n + g.dialogs.length, 0);

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
			{query.isError && !query.data ? (
				<ErrorState error={query.error} onRetry={() => void query.refetch()} />
			) : (
				<>
					<div className="flex flex-wrap items-center justify-between gap-3">
						<Tabs
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
							<TabsList>
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
						<SearchInput
							className="w-72 max-w-full"
							label="Search dialogs"
							onCommit={(value) =>
								void navigate({
									search: (prev) => ({ ...prev, q: value || undefined }),
									replace: true,
								})
							}
							placeholder="Search by title or id…"
							value={search.q ?? ""}
						/>
					</div>
					{!query.data ? (
						<div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
							{Array.from({ length: 9 }, (_, i) => (
								// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
								<Skeleton className="h-28 rounded-xl" key={i} />
							))}
						</div>
					) : groups.length === 0 ? (
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
									{groups.length.toLocaleString()} groups
								</p>
							)}
							<ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
								{groups.map((group) => (
									<GroupCard group={group} key={group.id || cat} />
								))}
							</ul>
						</>
					)}
				</>
			)}
		</div>
	);
}

function GroupCard({ group }: { group: StoryGroup }): React.ReactElement {
	const flat = !group.id;
	return (
		<li
			className={
				flat
					? "sm:col-span-2 lg:col-span-3"
					: "[contain-intrinsic-size:auto_9rem] [content-visibility:auto]"
			}
		>
			<div className="flex h-full flex-col gap-2.5 rounded-xl border bg-card p-3.5 shadow-xs/5">
				{!flat && (
					<div className="flex min-w-0 items-baseline justify-between gap-2">
						<span
							className="truncate font-medium text-sm"
							title={group.title ?? group.id}
						>
							{group.title || group.id}
						</span>
						<span className="shrink-0 font-mono text-muted-foreground text-xs">
							{group.title ? group.id : `${group.dialogs.length}`}
						</span>
					</div>
				)}
				<ul className="flex flex-wrap gap-1.5">
					{group.dialogs.map((dialog) => {
						const label = dialogShortLabel(dialog, group.id);
						return (
							<li key={dialog}>
								<Link
									className="inline-flex h-7 min-w-7 items-center justify-center rounded-md border bg-background px-2 font-mono text-xs outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring dark:bg-input/32"
									params={{ dialogId: dialog }}
									title={dialog}
									to="/story/$dialogId"
								>
									{label}
								</Link>
							</li>
						);
					})}
				</ul>
			</div>
		</li>
	);
}
