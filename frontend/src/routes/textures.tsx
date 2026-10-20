import {
	infiniteQueryOptions,
	useInfiniteQuery,
	useQuery,
} from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { FolderIcon, ImageIcon, SearchXIcon } from "lucide-react";
import * as React from "react";
import { FolderCards } from "@/components/gallery/folder-cards";
import { Lightbox } from "@/components/gallery/lightbox";
import { type GridItem, TextureGrid } from "@/components/gallery/texture-grid";
import { TextureHighlights } from "@/components/gallery/texture-highlights";
import { PageHeader } from "@/components/page-header";
import { SearchInput } from "@/components/search-input";
import { ErrorState } from "@/components/status-views";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/api";
import { basename, extname, formatNumber } from "@/lib/format";
import { treeQuery } from "@/lib/queries";
import { pageTitle } from "@/lib/route-meta";
import { searchParams, str, withDefault } from "@/lib/search-params";

const parseSearch = searchParams({
	dir: withDefault(str(), ""),
	q: str(),
	view: str(),
});

const PAGE = 240;

const textureSearch = (q: string) =>
	infiniteQueryOptions({
		queryKey: ["texture-search", q],
		queryFn: ({ pageParam, signal }) =>
			api.search(
				{ q, root: "textures", ext: "png", limit: PAGE, offset: pageParam },
				signal,
			),
		initialPageParam: 0,
		getNextPageParam: (last, pages) => {
			const loaded = pages.reduce((n, p) => n + p.items.length, 0);
			return loaded < last.total && last.items.length > 0 ? loaded : undefined;
		},
		staleTime: 5 * 60_000,
	});

export const Route = createFileRoute("/textures")({
	validateSearch: parseSearch,
	loaderDeps: ({ search }) => ({
		dir: search.dir,
		searching: Boolean(search.q),
	}),
	loader: async ({ context, deps }) => {
		if (!deps.searching) {
			await context.queryClient.query(treeQuery("textures", deps.dir));
		}
	},
	head: ({ match }) => {
		const { dir } = match.search as { dir?: string };
		return {
			meta: [{ title: pageTitle(dir ? basename(dir) : "", "Textures") }],
		};
	},
	staticData: { section: "Assets", crumb: "Textures" },
	component: TexturesPage,
});

const getScroller = () => document.getElementById("content");

function TexturesPage(): React.ReactElement {
	const { dir, q, view } = Route.useSearch();
	const navigate = Route.useNavigate();
	const query = (q ?? "").trim();
	const searching = query.length > 0;

	const tree = useQuery({ ...treeQuery("textures", dir), enabled: !searching });
	const search = useInfiniteQuery({
		...textureSearch([dir, query].filter(Boolean).join(" ")),
		enabled: searching,
	});

	const folders = React.useMemo(
		() =>
			searching
				? []
				: (tree.data?.entries ?? []).filter((e) => e.kind === "dir"),
		[searching, tree.data],
	);

	const items = React.useMemo<GridItem[]>(() => {
		if (searching) {
			return (search.data?.pages ?? []).flatMap((p) =>
				p.items.map((it) => ({ path: it.path, name: it.name })),
			);
		}
		return (tree.data?.entries ?? [])
			.filter((e) => e.kind === "file" && extname(e.name) === "png")
			.map((e) => ({ path: e.path, name: e.name }));
	}, [searching, search.data, tree.data]);

	const total = searching ? (search.data?.pages[0]?.total ?? 0) : items.length;
	const active = searching ? search : tree;

	const viewIndex = view ? items.findIndex((it) => it.path === view) : -1;
	const lightboxItems =
		view && viewIndex === -1 ? [{ path: view, name: basename(view) }] : items;

	const { hasNextPage, isFetchingNextPage, fetchNextPage } = search;
	const loadMore = React.useCallback(() => {
		if (searching && hasNextPage && !isFetchingNextPage) void fetchNextPage();
	}, [searching, hasNextPage, isFetchingNextPage, fetchNextPage]);

	return (
		<div className="flex w-full flex-col gap-4 p-4 md:p-6">
			<PageHeader
				actions={
					<SearchInput
						className="w-72 max-w-full"
						label="Search textures"
						onCommit={(value) =>
							void navigate({
								search: (prev) => ({
									...prev,
									q: value || undefined,
									view: undefined,
								}),
								replace: true,
							})
						}
						placeholder={
							dir ? `Search in ${basename(dir)}…` : "Search all textures…"
						}
						value={q ?? ""}
					/>
				}
				description={
					active.isPending
						? "Loading…"
						: searching
							? `${formatNumber(total)} textures match “${query}”${dir ? ` in ${basename(dir)}` : ""}`
							: items.length === 0 && folders.length > 0
								? `${formatNumber(folders.length)} folders`
								: `${formatNumber(folders.length)} folders · ${formatNumber(items.length)} textures in this folder`
				}
				icon={<ImageIcon />}
				title="Textures"
			/>

			<FolderCrumbs dir={dir} />

			{!searching && folders.length > 0 && (
				<FolderCards compact={items.length > 0} folders={folders} />
			)}

			{active.isError ? (
				<ErrorState
					error={active.error}
					onRetry={() => void active.refetch()}
				/>
			) : active.isPending ? (
				<div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-2">
					{Array.from({ length: 24 }, (_, i) => (
						// biome-ignore lint/suspicious/noArrayIndexKey: placeholders
						<Skeleton className="aspect-square rounded-lg" key={i} />
					))}
				</div>
			) : items.length === 0 && !searching && folders.length > 0 ? (
				dir === "" ? (
					<TextureHighlights
						onOpen={(path) =>
							void navigate({ search: (prev) => ({ ...prev, view: path }) })
						}
					/>
				) : null
			) : items.length === 0 ? (
				<Empty>
					<EmptyHeader>
						<EmptyMedia variant="icon">
							{searching ? <SearchXIcon /> : <FolderIcon />}
						</EmptyMedia>
						<EmptyTitle>
							{searching ? "No textures found" : "No textures here"}
						</EmptyTitle>
						<EmptyDescription>
							{searching
								? "Try fewer or different words. Each word must appear somewhere in the path."
								: "This folder is empty."}
						</EmptyDescription>
					</EmptyHeader>
				</Empty>
			) : (
				<>
					<TextureGrid
						activePath={view}
						items={items}
						onEndReached={loadMore}
						onOpen={(index) =>
							void navigate({
								search: (prev) => ({ ...prev, view: items[index]?.path }),
							})
						}
						scrollElement={getScroller}
					/>
					{searching && (hasNextPage || isFetchingNextPage) && (
						<div className="flex justify-center py-4">
							<Button
								loading={isFetchingNextPage}
								onClick={() => void fetchNextPage()}
								variant="outline"
							>
								Load more
							</Button>
						</div>
					)}
					{isFetchingNextPage && <Spinner className="mx-auto" />}
				</>
			)}

			<Lightbox
				index={view ? Math.max(0, viewIndex) : null}
				items={lightboxItems}
				total={lightboxItems === items ? total : undefined}
				onClose={() =>
					void navigate({ search: (prev) => ({ ...prev, view: undefined }) })
				}
				onIndexChange={(index) =>
					void navigate({
						search: (prev) => ({ ...prev, view: lightboxItems[index]?.path }),
						replace: true,
					})
				}
			/>
		</div>
	);
}

function FolderCrumbs({ dir }: { dir: string }): React.ReactElement | null {
	const segments = dir ? dir.split("/") : [];
	return (
		<nav aria-label="Texture folder">
			<ol className="flex flex-wrap items-center gap-1 text-sm">
				<li>
					<Link
						className="rounded-sm px-1 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring aria-[current=page]:font-medium aria-[current=page]:text-foreground"
						search={{ dir: "" }}
						to="/textures"
						aria-current={segments.length === 0 ? "page" : undefined}
					>
						All textures
					</Link>
				</li>
				{segments.map((segment, i) => {
					const isLast = i === segments.length - 1;
					return (
						<li
							className="flex items-center gap-1"
							key={segments.slice(0, i + 1).join("/")}
						>
							<span aria-hidden="true" className="text-muted-foreground/60">
								/
							</span>
							<Link
								aria-current={isLast ? "page" : undefined}
								className="rounded-sm px-0.5 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring aria-[current=page]:font-medium aria-[current=page]:text-foreground"
								search={{ dir: segments.slice(0, i + 1).join("/") }}
								to="/textures"
							>
								{segment}
							</Link>
						</li>
					);
				})}
			</ol>
		</nav>
	);
}
