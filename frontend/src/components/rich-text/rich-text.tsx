import * as React from "react";
import { Kbd } from "@/components/ui/kbd";
import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";
import { thumbUrl } from "@/lib/api";
import type { Rich, RichSegment, Terms } from "@/lib/catalog";
import { cn } from "@/lib/utils";

const TermsContext = React.createContext<Terms | null>(null);

/** Provides the response's `terms` glossary to every nested `<RichText>`. */
export function TermsProvider({
	terms,
	children,
}: {
	terms: Terms | null | undefined;
	children: React.ReactNode;
}): React.ReactElement {
	return (
		<TermsContext.Provider value={terms ?? null}>
			{children}
		</TermsContext.Provider>
	);
}

/** `<p="reading" padding=0>base</p>` furigana markup (JP) left in the text. */
const RUBY = /<p="([^"]*)"[^>]*>(.*?)<\/p>/g;

/** Renders plain text, turning furigana markup into `<ruby>`. */
function Text({ text }: { text: string }): React.ReactElement {
	if (!text.includes("<p=")) return <>{text}</>;
	const out: React.ReactNode[] = [];
	let last = 0;
	for (const match of text.matchAll(RUBY)) {
		const at = match.index ?? 0;
		if (at > last) out.push(text.slice(last, at));
		out.push(
			<ruby key={at}>
				{match[2]}
				<rp>(</rp>
				<rt className="text-[0.6em] text-muted-foreground">{match[1]}</rt>
				<rp>)</rp>
			</ruby>,
		);
		last = at + match[0].length;
	}
	if (last < text.length) out.push(text.slice(last));
	return <>{out}</>;
}

/**
 * Game colours are tuned for dark UI. In light mode the lightness is capped
 * (relative colour syntax) so pale tints stay readable on white.
 */
const COLOR_CLASS =
	"[color:oklch(from_var(--seg)_min(l,0.5)_max(c,0.06)_h)] dark:[color:var(--seg)]";

/** Style ids that read as emphasised numbers / keywords. */
const STRONG_STYLES = new Set(["ba.vup", "ba.vdown", "ba.key"]);

function segmentClass(segment: RichSegment): string {
	return cn(
		segment.color && COLOR_CLASS,
		segment.style && "font-medium",
		segment.style && STRONG_STYLES.has(segment.style) && "tabular-nums",
		segment.bold && "font-semibold",
		segment.italic && "italic",
	);
}

function inlineTokenLabel(ref: string): string {
	const last = ref.split("/").pop() ?? ref;
	return last
		.replace(/^icon_(key_)?/, "")
		.replace(/_/g, " ")
		.trim();
}

function Segment({
	segment,
	nested,
}: {
	segment: RichSegment;
	nested: boolean;
}): React.ReactElement | null {
	const terms = React.useContext(TermsContext);
	if (segment.image || segment.action) {
		const ref = (segment.image ?? segment.action) as string;
		return (
			<Kbd className="mx-0.5 align-[0.1em] capitalize" title={ref}>
				{inlineTokenLabel(ref) || ref}
			</Kbd>
		);
	}
	if (!segment.text) return null;
	const style = segment.color
		? ({ "--seg": segment.color } as React.CSSProperties)
		: undefined;
	const term = segment.term ? terms?.[segment.term] : undefined;
	if (term && !nested) {
		return (
			<TermPopover termId={segment.term as string} terms={terms as Terms}>
				<span className={segmentClass(segment)} style={style}>
					<Text text={segment.text} />
				</span>
			</TermPopover>
		);
	}
	if (!segment.color && !segment.style && !segment.bold && !segment.italic) {
		return <Text text={segment.text} />;
	}
	return (
		<span
			className={cn(
				segmentClass(segment),
				segment.term && "underline decoration-current/40 decoration-dotted",
			)}
			data-style={segment.style}
			style={style}
		>
			<Text text={segment.text} />
		</span>
	);
}

function TermPopover({
	termId,
	terms,
	children,
}: {
	termId: string;
	terms: Terms;
	children: React.ReactNode;
}): React.ReactElement {
	const term = terms[termId];
	if (!term) return <>{children}</>;
	const style = term.color
		? ({ "--seg": term.color } as React.CSSProperties)
		: undefined;
	return (
		<Popover>
			<PopoverTrigger
				className="inline cursor-help rounded-[3px] bg-transparent p-0 text-start align-baseline font-[inherit] text-inherit underline decoration-current/50 decoration-dotted underline-offset-[3px] outline-none transition-colors hover:decoration-solid focus-visible:ring-2 focus-visible:ring-ring"
				closeDelay={80}
				delay={250}
				openOnHover
			>
				{children}
			</PopoverTrigger>
			<PopoverPopup className="w-80 max-w-[calc(100vw-2rem)]" side="top">
				<div className="flex flex-col gap-2">
					<div className="flex items-center gap-2">
						{term.icon && (
							<img
								alt=""
								className="size-6 shrink-0 rounded-sm bg-foreground/80 object-contain p-0.5 dark:bg-transparent dark:p-0"
								src={thumbUrl(term.icon, 128)}
							/>
						)}
						<span
							className={cn("font-semibold text-sm", term.color && COLOR_CLASS)}
							style={style}
						>
							{term.name}
						</span>
					</div>
					<RichText
						className="text-muted-foreground text-sm leading-relaxed"
						nested
						value={term.description}
					/>
				</div>
			</PopoverPopup>
		</Popover>
	);
}

/**
 * Renders the API's rich-text segment arrays. `\n` inside segments becomes a
 * line break (`white-space: pre-line`). Term segments open a glossary popover
 * (hover or click / Enter) when a `<TermsProvider>` supplies the definition.
 */
export function RichText({
	value,
	className,
	nested = false,
	as: Tag = "span",
}: {
	value: Rich | string | null | undefined;
	className?: string;
	/** Inside a term popover: terms render as plain styled text. */
	nested?: boolean;
	as?: "span" | "p" | "div";
}): React.ReactElement | null {
	if (!value || value.length === 0) return null;
	return (
		<Tag className={cn("whitespace-pre-line", className)}>
			{typeof value === "string" ? (
				<Text text={value} />
			) : (
				value.map((segment, i) => (
					// biome-ignore lint/suspicious/noArrayIndexKey: segments are positional and immutable
					<Segment key={i} nested={nested} segment={segment} />
				))
			)}
		</Tag>
	);
}
