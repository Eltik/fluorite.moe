import { useVirtualizer } from "@tanstack/react-virtual";
import { WrapTextIcon } from "lucide-react";
import * as React from "react";
import type { ThemedToken } from "shiki/core";
import { Toggle } from "@/components/ui/toggle";
import { formatNumber } from "@/lib/format";
import { cn } from "@/lib/utils";
import { type HighlightLang, tokenize } from "./highlighter";

/** Above these sizes highlighting is skipped; the file still renders. */
const HIGHLIGHT_MAX_BYTES = 400_000;
const HIGHLIGHT_MAX_LINES = 12_000;
const LINE_HEIGHT = 20;

export interface CodeViewerProps {
	code: string;
	lang?: HighlightLang | null;
	className?: string;
}

export default function CodeViewer({
	code,
	lang = null,
	className,
}: CodeViewerProps): React.ReactElement {
	const lines = React.useMemo(() => code.split(/\r?\n/), [code]);
	const [tokens, setTokens] = React.useState<ThemedToken[][] | null>(null);
	const [wrap, setWrap] = React.useState(false);
	const tooBig =
		code.length > HIGHLIGHT_MAX_BYTES || lines.length > HIGHLIGHT_MAX_LINES;

	React.useEffect(() => {
		setTokens(null);
		if (!lang || tooBig) return;
		let cancelled = false;
		// Yield to paint the plain text first, then highlight.
		const id = window.setTimeout(() => {
			tokenize(code, lang)
				.then((result) => {
					if (!cancelled) setTokens(result);
				})
				.catch(() => undefined);
		}, 0);
		return () => {
			cancelled = true;
			window.clearTimeout(id);
		};
	}, [code, lang, tooBig]);

	const scrollRef = React.useRef<HTMLDivElement>(null);
	const virtualizer = useVirtualizer({
		count: lines.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: () => LINE_HEIGHT,
		overscan: 20,
		measureElement: wrap
			? (el) => el.getBoundingClientRect().height
			: undefined,
	});

	// Re-measure when wrapping is toggled.
	// biome-ignore lint/correctness/useExhaustiveDependencies: wrap is the trigger
	React.useEffect(() => {
		virtualizer.measure();
	}, [wrap, virtualizer]);

	const gutter = `${String(lines.length).length + 1}ch`;

	return (
		<div className={cn("flex min-h-0 flex-1 flex-col", className)}>
			<div className="flex h-9 shrink-0 items-center gap-2 border-b px-3 text-muted-foreground text-xs">
				<span className="tabular-nums">{formatNumber(lines.length)} lines</span>
				{lang && (
					<span>
						·{" "}
						{tooBig
							? "too large to highlight"
							: tokens
								? lang
								: "highlighting…"}
					</span>
				)}
				<Toggle
					aria-label="Wrap lines"
					className="ms-auto"
					onPressedChange={setWrap}
					pressed={wrap}
					size="sm"
				>
					<WrapTextIcon />
				</Toggle>
			</div>
			<div
				className="min-h-0 flex-1 overflow-auto bg-code font-mono text-[0.8125rem] leading-5"
				ref={scrollRef}
				// biome-ignore lint/a11y/noNoninteractiveTabindex: scrollable region must be keyboard reachable
				tabIndex={0}
			>
				<div
					className={cn("relative", !wrap && "w-max min-w-full")}
					style={{ height: virtualizer.getTotalSize() }}
				>
					{virtualizer.getVirtualItems().map((item) => (
						<div
							className="absolute top-0 left-0 flex w-full hover:bg-code-highlight"
							data-index={item.index}
							key={item.key}
							ref={wrap ? virtualizer.measureElement : undefined}
							style={{
								transform: `translateY(${item.start}px)`,
								minHeight: LINE_HEIGHT,
							}}
						>
							<span
								aria-hidden="true"
								className="sticky left-0 shrink-0 select-none border-r bg-code pe-3 text-end text-muted-foreground/60 tabular-nums"
								style={{
									width: gutter,
									minWidth: gutter,
									boxSizing: "content-box",
								}}
							>
								{item.index + 1}
							</span>
							<span
								className={cn(
									"ps-4 pe-6",
									wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre",
								)}
							>
								<Line
									text={lines[item.index] ?? ""}
									tokens={tokens?.[item.index]}
								/>
							</span>
						</div>
					))}
				</div>
			</div>
		</div>
	);
}

function Line({
	text,
	tokens,
}: {
	text: string;
	tokens?: ThemedToken[];
}): React.ReactNode {
	if (!tokens) return text || " ";
	return tokens.map((token, i) => (
		<span
			className="shiki-token"
			// biome-ignore lint/suspicious/noArrayIndexKey: tokens are positional
			key={i}
			style={token.htmlStyle as React.CSSProperties | undefined}
		>
			{token.content}
		</span>
	));
}
