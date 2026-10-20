import * as React from "react";
import { RichText } from "@/components/rich-text/rich-text";
import { type Rich, richToPlain, titleCaseShouting } from "@/lib/catalog";
import { cn } from "@/lib/utils";

type Block =
	| { type: "heading"; text: string }
	| { type: "facts"; items: { label: string; value: string }[] }
	| { type: "para"; text: string };

/** A line that is only a bracketed title: `[ORIPATHY INFECTION STATUS]`, `【档案】`. */
const HEADING = /^\s*[[【［]\s*([^\]】］]{1,60}?)\s*[\]】］]\s*$/;
/** `CODENAME: Chen Qianyu`, `代号：陈千语`. */
const FACT = /^\s*([^:：.!?。！？,，]{1,40}?)\s*[:：]\s*(\S.*)$/;

function isFactLabel(label: string): boolean {
	// Short labels only; Latin labels must be shouted (the game's convention),
	// so ordinary sentences with a colon stay prose.
	if (label.split(/\s+/).length > 5) return false;
	return /[A-Za-z]/.test(label) ? label === label.toUpperCase() : true;
}

/** Splits profile text into headings, label/value runs and paragraphs. */
export function parseProfile(text: string): Block[] | null {
	const blocks: Block[] = [];
	let para: string[] = [];
	let structured = false;
	const flush = () => {
		const joined = para.join("\n").trim();
		if (joined) blocks.push({ type: "para", text: joined });
		para = [];
	};
	for (const raw of text.split(/\r?\n/)) {
		const line = raw.trimEnd();
		const heading = HEADING.exec(line);
		if (heading?.[1]) {
			flush();
			blocks.push({ type: "heading", text: heading[1] });
			structured = true;
			continue;
		}
		const fact = FACT.exec(line);
		if (fact?.[1] && fact[2] && isFactLabel(fact[1])) {
			flush();
			const last = blocks.at(-1);
			const item = { label: fact[1], value: fact[2].trim() };
			if (last?.type === "facts") last.items.push(item);
			else blocks.push({ type: "facts", items: [item] });
			structured = true;
			continue;
		}
		if (!line.trim()) {
			flush();
			continue;
		}
		para.push(line);
	}
	flush();
	return structured ? blocks : null;
}

/** Whether the rich value is plain text (no colours, terms or sprites). */
function isPlain(rich: Rich): boolean {
	return rich.every(
		(s) => !s.style && !s.color && !s.term && !s.image && !s.action,
	);
}

/**
 * Profile entry text. Game "file" text (`CODENAME: …` lines and `[SECTION]`
 * titles) becomes a definition list with sub-headings; anything else renders
 * as rich text. Prose is capped at a readable measure.
 */
export function ProfileText({
	value,
	className,
}: {
	value: Rich;
	className?: string;
}): React.ReactElement {
	const blocks = React.useMemo(
		() => (isPlain(value) ? parseProfile(richToPlain(value)) : null),
		[value],
	);
	if (!blocks) {
		return (
			<RichText
				as="p"
				className={cn(
					"max-w-[70ch] whitespace-pre-line text-pretty text-foreground/90 text-sm leading-relaxed",
					className,
				)}
				value={value}
			/>
		);
	}
	return (
		<div className={cn("flex max-w-[70ch] flex-col gap-3", className)}>
			{blocks.map((block, i) => {
				const key = `${block.type}-${i}`;
				if (block.type === "heading") {
					return (
						<h4
							className="mt-1 font-medium text-foreground text-sm first:mt-0"
							key={key}
						>
							{sentenceCase(block.text)}
						</h4>
					);
				}
				if (block.type === "facts") {
					return (
						<dl
							className="grid grid-cols-[minmax(6rem,11rem)_1fr] gap-x-4 gap-y-1.5 text-sm"
							key={key}
						>
							{block.items.map((item) => (
								<React.Fragment key={item.label}>
									<dt className="text-muted-foreground">
										{sentenceCase(item.label)}
									</dt>
									<dd className="min-w-0 text-foreground/90">{item.value}</dd>
								</React.Fragment>
							))}
						</dl>
					);
				}
				return (
					<p
						className="whitespace-pre-line text-pretty text-foreground/90 text-sm leading-relaxed"
						key={key}
					>
						{block.text}
					</p>
				);
			})}
		</div>
	);
}

/** `ORIPATHY INFECTION STATUS` → `Oripathy infection status`. */
function sentenceCase(text: string): string {
	if (/^[A-Z]{2,3}$/.test(text.trim())) return text;
	const t = titleCaseShouting(text);
	if (t === text) return text;
	const lower = t.toLowerCase();
	return lower.charAt(0).toUpperCase() + lower.slice(1);
}
