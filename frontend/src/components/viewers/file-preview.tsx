import { useQuery } from "@tanstack/react-query";
import {
	CopyIcon,
	DownloadIcon,
	ExternalLinkIcon,
	FileIcon,
	FileTextIcon,
	TriangleAlertIcon,
} from "lucide-react";
import * as React from "react";
import { ErrorState } from "@/components/status-views";
import { Button } from "@/components/ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Spinner } from "@/components/ui/spinner";
import { fileUrl, type Root } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import {
	basename,
	extname,
	formatBytes,
	type PreviewKind,
	previewKind,
} from "@/lib/format";
import { parseJsonSafe } from "@/lib/json-parse";
import { fileTextQuery } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { HighlightLang } from "./highlighter";
import { ImageViewer } from "./image-viewer";

const JsonViewer = React.lazy(() => import("./json-viewer"));
const CodeViewer = React.lazy(() => import("./code-viewer"));
const ModelViewer = React.lazy(() => import("./model-viewer"));

/** Above this, text previews wait for an explicit click. */
const LARGE_TEXT_BYTES = 24 * 1024 * 1024;
const TEXT_FALLBACK_MAX = 4 * 1024 * 1024;
/** How much of a large file the "first part" preview fetches (one Range request). */
const HEAD_BYTES = 1024 * 1024;

/** First `HEAD_BYTES` of a file, cut back to the last complete line. */
async function fetchHead(
	root: Root,
	path: string,
	signal: AbortSignal,
): Promise<string> {
	const res = await fetch(fileUrl(root, path), {
		headers: { Range: `bytes=0-${HEAD_BYTES - 1}` },
		signal,
	});
	if (!res.ok) throw new Error(`Request failed (${res.status})`);
	const text = await res.text();
	const cut = text.lastIndexOf("\n");
	return cut > 0 ? text.slice(0, cut) : text;
}

function langFor(name: string): HighlightLang | null {
	const ext = extname(name);
	if (ext === "lua") return "lua";
	if (ext === "json") return "json";
	if (ext === "xml") return "xml";
	return null;
}

export function ViewerFallback(): React.ReactElement {
	return (
		<div className="flex flex-1 items-center justify-center">
			<Spinner />
		</div>
	);
}

export function FilePreview({
	root,
	path,
	size,
	className,
	headerStart,
}: {
	root: Root;
	path: string;
	size?: number;
	className?: string;
	/** Slot before the file name (e.g. a mobile back button). */
	headerStart?: React.ReactNode;
}): React.ReactElement {
	const name = basename(path);
	const url = fileUrl(root, path);
	const [forceText, setForceText] = React.useState(false);
	const kind: PreviewKind = forceText ? "code" : previewKind(name);

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset per file
	React.useEffect(() => setForceText(false), [path]);

	return (
		<section
			aria-label={`Preview of ${name}`}
			className={cn("flex min-h-0 min-w-0 flex-1 flex-col", className)}
		>
			<header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
				{headerStart}
				<div className="flex min-w-0 flex-1 items-baseline gap-2">
					<h2 className="truncate font-medium text-sm" title={path}>
						{name}
					</h2>
					{size !== undefined && (
						<span className="shrink-0 text-muted-foreground text-xs tabular-nums">
							{formatBytes(size)}
						</span>
					)}
				</div>
				<Button
					aria-label="Copy file path"
					onClick={() => void copyText(path, "File path")}
					size="icon-sm"
					title="Copy file path"
					variant="ghost"
				>
					<CopyIcon />
				</Button>
				<Button
					aria-label="Open raw file in a new tab"
					render={<a href={url} rel="noreferrer" target="_blank" />}
					size="icon-sm"
					title="Open raw"
					variant="ghost"
				>
					<ExternalLinkIcon />
				</Button>
				<Button
					aria-label="Download file"
					render={<a download={name} href={url} />}
					size="icon-sm"
					title="Download"
					variant="ghost"
				>
					<DownloadIcon />
				</Button>
			</header>
			<PreviewBody
				kind={kind}
				name={name}
				onForceText={() => setForceText(true)}
				path={path}
				root={root}
				size={size}
				url={url}
			/>
		</section>
	);
}

function PreviewBody({
	kind,
	root,
	path,
	name,
	size,
	url,
	onForceText,
}: {
	kind: PreviewKind;
	root: Root;
	path: string;
	name: string;
	size?: number;
	url: string;
	onForceText: () => void;
}): React.ReactElement {
	switch (kind) {
		case "image":
			return (
				<ImageViewer alt={name} bytes={size} downloadName={name} src={url} />
			);
		case "model":
			return (
				<React.Suspense fallback={<ViewerFallback />}>
					<ModelViewer url={url} />
				</React.Suspense>
			);
		case "json":
		case "code":
			return (
				<TextPreview
					kind={kind}
					lang={langFor(name)}
					path={path}
					root={root}
					size={size}
				/>
			);
		default:
			return (
				<Empty>
					<EmptyHeader>
						<EmptyMedia variant="icon">
							<FileIcon />
						</EmptyMedia>
						<EmptyTitle>
							No preview for .{extname(name) || "bin"} files
						</EmptyTitle>
						<EmptyDescription>
							{formatBytes(size)} · {path}
						</EmptyDescription>
					</EmptyHeader>
					<EmptyContent className="flex-row justify-center gap-2">
						<Button render={<a download={name} href={url} />}>
							<DownloadIcon />
							Download
						</Button>
						{(size ?? 0) <= TEXT_FALLBACK_MAX && (
							<Button onClick={onForceText} variant="outline">
								<FileTextIcon />
								View as text
							</Button>
						)}
					</EmptyContent>
				</Empty>
			);
	}
}

function TextPreview({
	kind,
	root,
	path,
	size,
	lang,
}: {
	kind: "json" | "code";
	root: Root;
	path: string;
	size?: number;
	lang: HighlightLang | null;
}): React.ReactElement {
	const large = (size ?? 0) > LARGE_TEXT_BYTES;
	const [mode, setMode] = React.useState<"gate" | "head" | "full">(
		large ? "gate" : "full",
	);
	const gated = mode === "gate";
	const query = useQuery({
		...fileTextQuery(root, path),
		enabled: mode === "full",
	});
	const head = useQuery({
		queryKey: ["file-head", root, path],
		queryFn: ({ signal }) => fetchHead(root, path, signal),
		enabled: mode === "head",
		staleTime: 10 * 60_000,
	});

	const parsed = React.useMemo(() => {
		if (kind !== "json" || query.data === undefined) return null;
		try {
			return { ok: true as const, value: parseJsonSafe(query.data) };
		} catch (error) {
			return {
				ok: false as const,
				message: error instanceof Error ? error.message : String(error),
			};
		}
	}, [kind, query.data]);

	if (gated) {
		return (
			<Empty>
				<EmptyHeader>
					<EmptyMedia variant="icon">
						<TriangleAlertIcon />
					</EmptyMedia>
					<EmptyTitle>Large file</EmptyTitle>
					<EmptyDescription>
						This file is {formatBytes(size)}. Loading all of it may take a
						while; the first {formatBytes(HEAD_BYTES)} opens instantly as plain
						text.
					</EmptyDescription>
				</EmptyHeader>
				<EmptyContent className="flex-row justify-center">
					<Button onClick={() => setMode("head")}>
						Show first {formatBytes(HEAD_BYTES)}
					</Button>
					<Button onClick={() => setMode("full")} variant="outline">
						Load full file
					</Button>
				</EmptyContent>
			</Empty>
		);
	}
	if (mode === "head") {
		if (head.isError) {
			return (
				<ErrorState error={head.error} onRetry={() => void head.refetch()} />
			);
		}
		if (head.data === undefined) return <ViewerFallback />;
		return (
			<div className="flex min-h-0 flex-1 flex-col">
				<div className="flex shrink-0 items-center gap-2 border-b bg-muted/40 px-3 py-1.5 text-muted-foreground text-xs">
					<FileTextIcon className="size-3.5" />
					Showing the first {formatBytes(HEAD_BYTES)} of {formatBytes(size)} as
					text.
					<Button
						className="ms-auto"
						onClick={() => setMode("full")}
						size="xs"
						variant="outline"
					>
						Load full file
					</Button>
				</div>
				<React.Suspense fallback={<ViewerFallback />}>
					<CodeViewer code={head.data} key={`${path}#head`} lang={lang} />
				</React.Suspense>
			</div>
		);
	}
	if (query.isError) {
		return (
			<ErrorState error={query.error} onRetry={() => void query.refetch()} />
		);
	}
	if (query.data === undefined) {
		return (
			<div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground text-sm">
				<Spinner />
				{size !== undefined &&
					size > 1_000_000 &&
					`Loading ${formatBytes(size)}…`}
			</div>
		);
	}

	if (kind === "json" && parsed?.ok) {
		return (
			<React.Suspense fallback={<ViewerFallback />}>
				<JsonViewer key={path} text={query.data} value={parsed.value} />
			</React.Suspense>
		);
	}

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			{parsed && !parsed.ok && (
				<div className="flex shrink-0 items-center gap-2 border-b bg-warning/8 px-3 py-1.5 text-warning-foreground text-xs">
					<TriangleAlertIcon className="size-3.5" />
					Not valid JSON ({parsed.message}). Showing as text.
				</div>
			)}
			<React.Suspense fallback={<ViewerFallback />}>
				<CodeViewer code={query.data} key={path} lang={lang} />
			</React.Suspense>
		</div>
	);
}
