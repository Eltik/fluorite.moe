import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, notFound } from "@tanstack/react-router";
import { ArrowLeftIcon, FolderOpenIcon } from "lucide-react";
import * as React from "react";
import { z } from "zod";
import {
	DirectoryList,
	type DirectoryListHandle,
} from "@/components/browser/directory-list";
import { SearchInput } from "@/components/search-input";
import { ErrorState, ListSkeleton } from "@/components/status-views";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { FilePreview } from "@/components/viewers/file-preview";
import { isRoot, type Root, type TreeEntry } from "@/lib/api";
import { basename, dirname, formatNumber } from "@/lib/format";
import { ROOT_META } from "@/lib/nav";
import { treeQuery } from "@/lib/queries";
import { pushRecent } from "@/lib/recent";
import { pageTitle } from "@/lib/route-meta";
import { cn } from "@/lib/utils";

const searchSchema = z.object({
	path: z.string().catch("").default(""),
	file: z.string().optional().catch(undefined),
	q: z.string().optional().catch(undefined),
});

export const Route = createFileRoute("/browse/$root")({
	validateSearch: searchSchema,
	params: {
		parse: ({ root }) => {
			if (!isRoot(root)) throw notFound();
			return { root };
		},
		stringify: ({ root }) => ({ root }),
	},
	loaderDeps: ({ search }) => ({ path: search.path }),
	loader: ({ context, params, deps }) =>
		context.queryClient.query(treeQuery(params.root, deps.path)),
	head: ({ params, match }) => {
		const search = match.search as { path?: string; file?: string };
		return {
			meta: [
				{
					title: pageTitle(
						search.file ? basename(search.file) : "",
						search.path ? basename(search.path) : "",
						ROOT_META[params.root].label,
					),
				},
			],
		};
	},
	staticData: {
		section: ({ root }) =>
			root === "textures" || root === "meshes" || root === "audio"
				? "Assets"
				: "Data",
		crumb: ({ root }) =>
			root && isRoot(root) ? ROOT_META[root].label : (root ?? ""),
	},
	pendingComponent: BrowsePending,
	component: BrowsePage,
});

function BrowsePending(): React.ReactElement {
	return (
		<div className="flex min-h-0 flex-1">
			<div className="w-full border-e md:w-80 lg:w-96">
				<ListSkeleton rows={18} />
			</div>
		</div>
	);
}

function BrowsePage(): React.ReactElement {
	const { root } = Route.useParams();
	const { path, file, q } = Route.useSearch();
	const navigate = Route.useNavigate();
	const tree = useQuery(treeQuery(root, path));
	const listRef = React.useRef<DirectoryListHandle>(null);
	const filter = (q ?? "").trim().toLowerCase();

	const entries = React.useMemo<TreeEntry[]>(() => {
		const all = tree.data?.entries ?? [];
		if (!filter) return all;
		return all.filter((e) => e.name.toLowerCase().includes(filter));
	}, [tree.data, filter]);

	const selected = React.useMemo(
		() => tree.data?.entries.find((e) => e.path === file),
		[tree.data, file],
	);

	React.useEffect(() => {
		if (file) pushRecent(root, file);
	}, [root, file]);

	const linkFor = React.useCallback(
		(entry: TreeEntry) =>
			entry.kind === "dir"
				? ({
						to: "/browse/$root",
						params: { root },
						search: { path: entry.path },
					} as const)
				: ({
						to: "/browse/$root",
						params: { root },
						search: { path, file: entry.path, q },
						replace: Boolean(file),
					} as const),
		[root, path, q, file],
	);

	const parentLink = path
		? ({
				to: "/browse/$root",
				params: { root },
				search: { path: dirname(path) },
			} as const)
		: undefined;

	const meta = ROOT_META[root];
	const truncated = tree.data && tree.data.total > tree.data.entries.length;

	return (
		<div className="flex min-h-0 flex-1">
			{/* Listing pane */}
			<div
				className={cn(
					"flex min-h-0 w-full shrink-0 flex-col border-e md:w-80 lg:w-96",
					file && "max-md:hidden",
				)}
			>
				<div className="flex shrink-0 flex-col gap-2 border-b p-2">
					<PathCrumbs path={path} root={root} />
					<SearchInput
						label={`Filter ${basename(path) || meta.label}`}
						onCommit={(value) =>
							void navigate({
								search: (prev) => ({ ...prev, q: value || undefined }),
								replace: true,
							})
						}
						onKeyDown={(event) => {
							if (event.key === "ArrowDown") {
								event.preventDefault();
								listRef.current?.focusFirst();
							}
						}}
						placeholder="Filter this folder…"
						value={q ?? ""}
					/>
				</div>
				{tree.isError ? (
					<ErrorState error={tree.error} onRetry={() => void tree.refetch()} />
				) : tree.isPending ? (
					<ListSkeleton rows={18} />
				) : (
					<DirectoryList
						emptyMessage={
							filter ? `Nothing in this folder matches “${q}”.` : undefined
						}
						entries={entries}
						label={`Contents of ${path || meta.label}`}
						linkFor={linkFor}
						parentLink={parentLink}
						ref={listRef}
						selectedPath={file}
					/>
				)}
				<div className="flex h-8 shrink-0 items-center justify-between gap-2 border-t px-3 text-muted-foreground text-xs tabular-nums">
					<span>
						{tree.data
							? filter
								? `${formatNumber(entries.length)} of ${formatNumber(tree.data.entries.length)}`
								: `${formatNumber(tree.data.total)} items`
							: " "}
					</span>
					{truncated && (
						<span>showing first {formatNumber(tree.data.entries.length)}</span>
					)}
				</div>
			</div>

			{/* Preview pane */}
			<div
				className={cn(
					"flex min-h-0 min-w-0 flex-1 flex-col",
					!file && "max-md:hidden",
				)}
			>
				{file ? (
					<FilePreview
						headerStart={
							<Button
								aria-label="Back to folder"
								className="md:hidden"
								render={
									<Link
										params={{ root }}
										search={{ path, q }}
										to="/browse/$root"
									/>
								}
								size="icon-sm"
								variant="ghost"
							>
								<ArrowLeftIcon />
							</Button>
						}
						key={file}
						path={file}
						root={root}
						size={selected?.size}
					/>
				) : (
					<Empty>
						<EmptyHeader>
							<EmptyMedia variant="icon">
								<FolderOpenIcon />
							</EmptyMedia>
							<EmptyTitle>{basename(path) || meta.label}</EmptyTitle>
							<EmptyDescription>
								{meta.description}. Pick a file to preview it, or press ⌘K to
								search every path.
							</EmptyDescription>
						</EmptyHeader>
					</Empty>
				)}
			</div>
		</div>
	);
}

function PathCrumbs({
	root,
	path,
}: {
	root: Root;
	path: string;
}): React.ReactElement {
	const segments = path ? path.split("/") : [];
	const meta = ROOT_META[root];
	const Icon = meta.icon;
	// Collapse the middle of deep paths, keeping the first and last two.
	const shown =
		segments.length > 4
			? [
					{ label: segments[0] ?? "", index: 0 },
					{ label: "…", index: -1 },
					...segments.slice(-2).map((label, i) => ({
						label,
						index: segments.length - 2 + i,
					})),
				]
			: segments.map((label, index) => ({ label, index }));

	return (
		<nav aria-label="Folder path" className="min-w-0">
			<ol className="flex min-w-0 items-center gap-1 text-sm">
				<li className="shrink-0">
					<Link
						className="flex items-center gap-1.5 rounded-sm px-1 font-medium outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
						params={{ root }}
						search={{ path: "" }}
						to="/browse/$root"
					>
						<Icon className="size-4 text-muted-foreground" />
						{meta.label}
					</Link>
				</li>
				{shown.map(({ label, index }) => {
					const isLast = index === segments.length - 1;
					return (
						<li
							className="flex min-w-0 items-center gap-1"
							key={`${index}-${label}`}
						>
							<span aria-hidden="true" className="text-muted-foreground/60">
								/
							</span>
							{index === -1 ? (
								<span className="text-muted-foreground" title={path}>
									…
								</span>
							) : isLast ? (
								<span
									aria-current="page"
									className="truncate font-medium"
									title={label}
								>
									{label}
								</span>
							) : (
								<Link
									className="truncate rounded-sm px-0.5 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
									params={{ root }}
									search={{ path: segments.slice(0, index + 1).join("/") }}
									title={label}
									to="/browse/$root"
								>
									{label}
								</Link>
							)}
						</li>
					);
				})}
			</ol>
		</nav>
	);
}
