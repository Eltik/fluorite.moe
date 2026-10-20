import { Link } from "@tanstack/react-router";
import {
	ArrowDownIcon,
	CornerDownRightIcon,
	FilmIcon,
	FlagIcon,
	GitBranchIcon,
	LinkIcon,
	NotebookTextIcon,
	RotateCcwIcon,
	SplitIcon,
	Undo2Icon,
} from "lucide-react";
import * as React from "react";
import { RichText } from "@/components/rich-text/rich-text";
import {
	advance,
	initialSteps,
	isSilent,
	mainNext,
	type NodeMap,
	nodeMap,
	pathTo,
	richLength,
	type Step,
	sideSummaries,
	sliceRich,
	waitingOn,
} from "@/components/story/story-graph";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { useMediaQuery } from "@/hooks/use-media-query";
import { thumbUrl } from "@/lib/api";
import {
	type Dialog,
	type DialogNode,
	type Rich,
	richToPlain,
} from "@/lib/catalog";
import { cn } from "@/lib/utils";

export type ReaderMode = "play" | "all";

export interface ReaderProps {
	dialog: Dialog;
	mode: ReaderMode;
	line: string | undefined;
	onLineChange: (line: string | undefined) => void;
	showAlt: boolean;
	typewriter: boolean;
}

/** Hash → hue, so each speaker keeps a stable avatar tint. */
function hue(name: string): number {
	let h = 0;
	for (const c of name) h = (h * 31 + (c.codePointAt(0) ?? 0)) % 360;
	return h;
}

function Avatar({ node }: { node: DialogNode }): React.ReactElement {
	const [failed, setFailed] = React.useState(false);
	const speaker = node.speaker;
	const name = speaker?.name ?? "";
	if (speaker?.icon && !failed) {
		return (
			<img
				alt=""
				className="size-10 shrink-0 rounded-full border bg-muted object-cover"
				loading="lazy"
				onError={() => setFailed(true)}
				src={thumbUrl(speaker.icon, 128)}
			/>
		);
	}
	return (
		<span
			aria-hidden
			className="flex size-10 shrink-0 select-none items-center justify-center rounded-full border font-semibold text-sm"
			style={{
				background: `oklch(0.9 0.05 ${hue(name)} / 0.5)`,
				color: `oklch(0.45 0.1 ${hue(name)})`,
			}}
		>
			{[...name][0] ?? "…"}
		</span>
	);
}

function Typewriter({
	value,
	onDone,
	skip,
}: {
	value: Rich;
	onDone: () => void;
	skip: boolean;
}): React.ReactElement {
	const total = richLength(value);
	const [shown, setShown] = React.useState(0);
	const doneRef = React.useRef(onDone);
	doneRef.current = onDone;
	React.useEffect(() => {
		if (skip) {
			setShown(total);
			return;
		}
		let frame = 0;
		const started = performance.now();
		const tick = (now: number) => {
			const n = Math.min(total, Math.floor(((now - started) / 1000) * 55));
			setShown(n);
			if (n < total) frame = requestAnimationFrame(tick);
		};
		frame = requestAnimationFrame(tick);
		return () => cancelAnimationFrame(frame);
	}, [total, skip]);
	React.useEffect(() => {
		if (shown >= total) doneRef.current();
	}, [shown, total]);
	return (
		<span className="relative block">
			{/* Full text reserves the final height and is what screen readers get. */}
			<RichText className="invisible block" value={value} />
			<span aria-hidden className="absolute inset-0">
				<RichText value={sliceRich(value, shown)} />
			</span>
		</span>
	);
}

function LineView({
	node,
	showAlt,
	highlight,
	typewriter,
	onTyped,
	skipTyping,
	onLink,
}: {
	node: DialogNode;
	showAlt: boolean;
	highlight?: boolean;
	typewriter?: boolean;
	onTyped?: () => void;
	skipTyping?: boolean;
	onLink?: () => void;
}): React.ReactElement {
	const text = node.text ?? [];
	const narration = !node.speaker?.name;
	return (
		<div
			className={cn(
				"group/line relative flex scroll-mt-24 gap-3 rounded-xl px-2 py-1.5 transition-colors",
				highlight && "bg-amber-500/10 ring-1 ring-amber-500/40",
				narration && "justify-center",
			)}
			data-node={node.id}
			id={`line-${node.id}`}
		>
			{!narration && <Avatar node={node} />}
			<div
				className={cn(
					"flex min-w-0 flex-col gap-1",
					narration ? "max-w-prose items-center text-center" : "flex-1",
				)}
			>
				{!narration && (
					<div className="flex items-baseline gap-2">
						<span className="font-medium text-sm">{node.speaker?.name}</span>
						{node.emotion && (
							<span className="text-[0.6875rem] text-muted-foreground/80">
								{node.emotion}
							</span>
						)}
					</div>
				)}
				<div
					className={cn(
						"w-fit max-w-full rounded-2xl text-[0.9375rem] leading-relaxed",
						narration
							? "text-muted-foreground italic"
							: "rounded-tl-sm border bg-card px-3.5 py-2 shadow-xs/5",
					)}
				>
					{typewriter && onTyped ? (
						<Typewriter
							onDone={onTyped}
							skip={Boolean(skipTyping)}
							value={text}
						/>
					) : (
						<RichText value={text} />
					)}
					{showAlt && node.altText && (
						<div className="mt-1.5 border-t border-dashed pt-1.5 text-muted-foreground text-sm">
							<span className="me-1.5 text-[0.6875rem] uppercase tracking-wide">
								Alt
							</span>
							<RichText value={node.altText} />
						</div>
					)}
				</div>
			</div>
			{onLink && (
				<button
					aria-label="Link to this line"
					className="absolute top-1.5 right-1 rounded-md p-1 text-muted-foreground opacity-0 outline-none transition-opacity hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring group-hover/line:opacity-100 pointer-coarse:hidden"
					onClick={onLink}
					type="button"
				>
					<LinkIcon className="size-3.5" />
				</button>
			)}
		</div>
	);
}

function Divider({
	icon: Icon,
	children,
	id,
	highlight,
}: {
	icon: React.ComponentType<{ className?: string }>;
	children: React.ReactNode;
	id?: string;
	highlight?: boolean;
}): React.ReactElement {
	return (
		<div
			className={cn(
				"flex scroll-mt-24 items-center gap-3 px-2 py-2 text-muted-foreground text-xs",
				highlight && "rounded-lg bg-amber-500/10",
			)}
			id={id ? `line-${id}` : undefined}
		>
			<span aria-hidden className="h-px flex-1 bg-border" />
			<span className="flex max-w-[80%] items-center gap-1.5 text-center">
				<Icon className="size-3.5 shrink-0" />
				<span className="min-w-0">{children}</span>
			</span>
			<span aria-hidden className="h-px flex-1 bg-border" />
		</div>
	);
}

function TransitionView({
	node,
	highlight,
	nodes,
	mode,
	onJump,
}: {
	node: DialogNode;
	highlight?: boolean;
	nodes: NodeMap;
	mode: ReaderMode;
	onJump?: (id: string) => void;
}): React.ReactElement | null {
	if (node.kind === "finish") {
		return (
			<Divider highlight={highlight} icon={FlagIcon} id={node.id}>
				End
			</Divider>
		);
	}
	switch (node.action) {
		case "summary":
			return (
				<div
					className={cn(
						"mx-2 my-1 flex scroll-mt-24 gap-2.5 rounded-xl border border-dashed bg-muted/40 px-3.5 py-2.5 text-muted-foreground text-sm",
						highlight && "ring-1 ring-amber-500/40",
					)}
					id={`line-${node.id}`}
				>
					<NotebookTextIcon className="mt-0.5 size-4 shrink-0" />
					<div className="flex flex-col gap-0.5">
						<span className="text-[0.6875rem] uppercase tracking-wide">
							Summary
						</span>
						<RichText className="text-pretty" value={node.text} />
					</div>
				</div>
			);
		case "cinematic":
			return (
				<Divider highlight={highlight} icon={FilmIcon} id={node.id}>
					Cinematic
					{node.ref ? <span className="font-mono"> · {node.ref}</span> : null}
				</Divider>
			);
		case "dialog":
			return (
				<Divider highlight={highlight} icon={CornerDownRightIcon} id={node.id}>
					Continues in{" "}
					{node.ref ? (
						<Link
							className="font-mono text-foreground underline-offset-2 hover:underline"
							params={{ dialogId: node.ref }}
							to="/story/$dialogId"
						>
							{node.ref}
						</Link>
					) : (
						"another dialog"
					)}
				</Divider>
			);
		case "transition":
			return (
				<Divider highlight={highlight} icon={ArrowDownIcon} id={node.id}>
					Scene transition
				</Divider>
			);
		default: {
			if (mode === "play") return null;
			const targets = mainNext(node, nodes);
			if (
				targets.length <= 1 &&
				node.action !== "if" &&
				node.action !== "branch"
			)
				return null;
			return (
				<Divider highlight={highlight} icon={GitBranchIcon} id={node.id}>
					{node.action === "if" ? "Condition" : "Branch"}
					{targets.length > 0 && (
						<>
							{" → "}
							{targets.map((t, i) => (
								<React.Fragment key={t}>
									{i > 0 && ", "}
									<JumpLink id={t} onJump={onJump} />
								</React.Fragment>
							))}
						</>
					)}
				</Divider>
			);
		}
	}
}

function JumpLink({
	id,
	onJump,
}: {
	id: string;
	onJump?: (id: string) => void;
}): React.ReactElement {
	return (
		<a
			className="font-mono text-foreground underline-offset-2 hover:underline"
			href={`#line-${id}`}
			onClick={(e) => {
				e.preventDefault();
				onJump?.(id);
			}}
		>
			#{id}
		</a>
	);
}

/** Preview text for a branch target (its first line). */
function branchPreview(id: string, nodes: NodeMap): string {
	let node = nodes.get(id);
	for (let i = 0; node && i < 8; i++) {
		if (node.kind === "line") {
			const text = richToPlain(node.text);
			const who = node.speaker?.name;
			return `${who ? `${who}: ` : ""}${text.length > 70 ? `${text.slice(0, 70)}…` : text}`;
		}
		if (node.kind === "finish") return "End the dialog";
		if (node.kind === "option") return "A choice";
		node = nodes.get(node.next[0] ?? "");
	}
	return `Node ${id}`;
}

// ---------------------------------------------------------------- reader

export function StoryReader(props: ReaderProps): React.ReactElement {
	return props.mode === "all" ? <ReadAll {...props} /> : <Play {...props} />;
}

function ReadAll({
	dialog,
	line,
	onLineChange,
	showAlt,
}: ReaderProps): React.ReactElement {
	const nodes = React.useMemo(() => nodeMap(dialog), [dialog]);
	const reduceMotion = useMediaQuery("(prefers-reduced-motion: reduce)");
	const jump = React.useCallback(
		(id: string) => {
			onLineChange(id);
			document.getElementById(`line-${id}`)?.scrollIntoView({
				behavior: reduceMotion ? "auto" : "smooth",
				block: "center",
			});
		},
		[onLineChange, reduceMotion],
	);
	// biome-ignore lint/correctness/useExhaustiveDependencies: scroll to the deep-linked line once per dialog
	React.useEffect(() => {
		if (!line) return;
		// After the router's own scroll restoration has run.
		const id = window.setTimeout(() => {
			document
				.getElementById(`line-${line}`)
				?.scrollIntoView({ block: "center" });
		}, 60);
		return () => window.clearTimeout(id);
	}, [dialog.id]);

	return (
		<div className="flex flex-col gap-1">
			{dialog.nodes.map((node) => {
				const highlight = node.id === line;
				if (node.kind === "line") {
					return (
						<LineView
							highlight={highlight}
							key={node.id}
							node={node}
							onLink={() => onLineChange(node.id)}
							showAlt={showAlt}
						/>
					);
				}
				if (node.kind === "option") {
					return (
						<div
							className={cn(
								"flex scroll-mt-24 flex-col items-end gap-1.5 px-2 py-1.5",
								highlight && "rounded-xl bg-amber-500/10",
							)}
							id={`line-${node.id}`}
							key={node.id}
						>
							<span className="flex items-center gap-1 text-[0.6875rem] text-muted-foreground uppercase tracking-wide">
								<SplitIcon className="size-3" />
								Choice
							</span>
							{node.options?.map((option) => {
								const to = option.next ?? node.next[0];
								return (
									<div
										className="flex max-w-[85%] items-start gap-2 rounded-2xl rounded-tr-sm border border-blue-500/24 bg-blue-500/8 px-3.5 py-2 text-[0.9375rem]"
										key={option.id}
									>
										<RichText value={option.text} />
										{to && (
											<span className="shrink-0 pt-0.5 text-xs">
												<JumpLink id={to} onJump={jump} />
											</span>
										)}
									</div>
								);
							})}
						</div>
					);
				}
				return (
					<TransitionView
						highlight={highlight}
						key={node.id}
						mode="all"
						node={node}
						nodes={nodes}
						onJump={jump}
					/>
				);
			})}
		</div>
	);
}

function isTypingTarget(target: EventTarget | null): boolean {
	if (!(target instanceof HTMLElement)) return false;
	return Boolean(
		target.closest(
			"input, textarea, select, button, a, [role=button], [role=menuitem], [contenteditable=true]",
		),
	);
}

function Play({
	dialog,
	line,
	onLineChange,
	showAlt,
	typewriter,
}: ReaderProps): React.ReactElement {
	const nodes = React.useMemo(() => nodeMap(dialog), [dialog]);
	const reduceMotion = useMediaQuery("(prefers-reduced-motion: reduce)");
	const animate = typewriter && !reduceMotion;

	const [steps, setSteps] = React.useState<Step[]>(() => {
		const deep = line ? pathTo(dialog, nodes, line) : null;
		return deep ?? initialSteps(dialog, nodes);
	});
	// Index of the step that is still being typed (-1 = none).
	const [typing, setTyping] = React.useState(-1);
	const [skip, setSkip] = React.useState(false);
	const endRef = React.useRef<HTMLDivElement>(null);
	const choicesRef = React.useRef<HTMLFieldSetElement>(null);

	const wait = waitingOn(steps, nodes);

	const push = React.useCallback(
		(next: Step[]) => {
			setSteps(next);
			const last = next[next.length - 1];
			const node = last ? nodes.get(last.id) : undefined;
			if (animate && node?.kind === "line") {
				setTyping(next.length - 1);
				setSkip(false);
			} else {
				setTyping(-1);
			}
		},
		[nodes, animate],
	);

	const next = React.useCallback(() => {
		if (typing !== -1 && !skip) {
			setSkip(true);
			return;
		}
		if (wait.type === "next") push(advance(steps, nodes));
		else if (wait.type === "option" || wait.type === "branch")
			choicesRef.current?.querySelector<HTMLElement>("button")?.focus();
	}, [typing, skip, wait.type, steps, nodes, push]);

	const back = React.useCallback(() => {
		setTyping(-1);
		setSteps((prev) => {
			if (prev.length <= 1) return prev;
			// Drop back to the previous visible step and clear its choice.
			let out = prev.slice(0, -1);
			while (out.length > 1) {
				const node = nodes.get(out[out.length - 1]?.id ?? "");
				if (node && node.kind === "transition" && isSilent(node))
					out = out.slice(0, -1);
				else break;
			}
			const last = out[out.length - 1];
			if (last && last.choice !== undefined) {
				out = [...out.slice(0, -1), { id: last.id }];
			}
			return out;
		});
	}, [nodes]);

	const choose = (choice: number) => {
		const updated = [...steps];
		const last = updated[updated.length - 1];
		if (!last) return;
		updated[updated.length - 1] = { ...last, choice };
		push(advance(updated, nodes));
	};

	const switchPath = (index: number, choice: number) => {
		const step = steps[index];
		if (!step || step.choice === choice) return;
		push(advance([...steps.slice(0, index), { ...step, choice }], nodes));
	};

	const restart = () => {
		push(initialSteps(dialog, nodes));
		onLineChange(undefined);
	};

	React.useEffect(() => {
		const onKey = (event: KeyboardEvent) => {
			if (event.metaKey || event.ctrlKey || event.altKey) return;
			if (isTypingTarget(event.target)) {
				// Buttons keep their own Space/Enter; number keys still pick choices.
				if (!/^[1-9]$/.test(event.key)) return;
			}
			if (
				event.key === " " ||
				event.key === "Enter" ||
				event.key === "ArrowRight"
			) {
				event.preventDefault();
				next();
			} else if (event.key === "ArrowLeft" || event.key === "Backspace") {
				event.preventDefault();
				back();
			} else if (/^[1-9]$/.test(event.key)) {
				const index = Number(event.key) - 1;
				if (wait.type === "option" && wait.node.options?.[index]) {
					event.preventDefault();
					choose(index);
				} else if (wait.type === "branch" && wait.targets[index]) {
					event.preventDefault();
					choose(index);
				}
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	});

	// biome-ignore lint/correctness/useExhaustiveDependencies: follow the transcript as it grows
	React.useEffect(() => {
		endRef.current?.scrollIntoView({
			behavior: reduceMotion ? "auto" : "smooth",
			block: "end",
		});
	}, [steps.length, reduceMotion]);

	const visibleLines = steps.filter(
		(s) => nodes.get(s.id)?.kind === "line",
	).length;
	const totalLines = dialog.nodes.filter((n) => n.kind === "line").length;

	return (
		<div className="flex flex-col gap-1">
			{steps.map((step, i) => {
				const node = nodes.get(step.id);
				if (!node) return null;
				const highlight = node.id === line;
				const extras = sideSummaries(node, nodes).map((s) => (
					<TransitionView key={s.id} mode="play" node={s} nodes={nodes} />
				));
				let body: React.ReactNode = null;
				if (node.kind === "line") {
					body = (
						<LineView
							highlight={highlight}
							node={node}
							onLink={() => onLineChange(node.id)}
							onTyped={i === typing ? () => setTyping(-1) : undefined}
							showAlt={showAlt}
							skipTyping={skip}
							typewriter={i === typing}
						/>
					);
				} else if (node.kind === "option") {
					const chosen =
						step.choice !== undefined ? node.options?.[step.choice] : undefined;
					body = chosen ? (
						<div className="flex justify-end px-2 py-1.5">
							<div className="max-w-[85%] rounded-2xl rounded-tr-sm bg-blue-600 px-3.5 py-2 text-[0.9375rem] text-white shadow-xs/5 dark:bg-blue-500">
								<RichText value={chosen.text} />
							</div>
						</div>
					) : null;
				} else if (
					step.choice !== undefined &&
					mainNext(node, nodes).length > 1
				) {
					const targets = mainNext(node, nodes);
					body = (
						<Divider icon={GitBranchIcon}>
							<span className="me-1.5">
								{node.action === "if" ? "Condition" : "Branch"} · path
							</span>
							<span className="inline-flex gap-1 align-middle">
								{targets.map((target, t) => (
									<button
										aria-label={`Follow path ${t + 1}: ${branchPreview(target, nodes)}`}
										aria-pressed={t === step.choice}
										className={cn(
											"inline-flex size-5 cursor-pointer items-center justify-center rounded-md border font-medium tabular-nums outline-none focus-visible:ring-2 focus-visible:ring-ring",
											t === step.choice
												? "border-foreground/30 bg-foreground text-background"
												: "hover:bg-accent hover:text-foreground",
										)}
										key={target}
										onClick={() => switchPath(i, t)}
										title={branchPreview(target, nodes)}
										type="button"
									>
										{t + 1}
									</button>
								))}
							</span>
						</Divider>
					);
				} else {
					body = (
						<TransitionView
							highlight={highlight}
							mode="play"
							node={node}
							nodes={nodes}
						/>
					);
				}
				return (
					// biome-ignore lint/suspicious/noArrayIndexKey: steps can revisit a node; position is identity
					<React.Fragment key={`${i}-${step.id}`}>
						{body}
						{extras}
					</React.Fragment>
				);
			})}

			<div className="sticky bottom-0 z-10 mt-4 -mx-1 flex flex-col gap-2 bg-gradient-to-t from-background via-background/95 to-transparent px-1 pt-6 pb-3">
				{(wait.type === "option" || wait.type === "branch") && typing === -1 ? (
					<fieldset
						className="m-0 flex min-w-0 flex-col items-end gap-1.5 border-0 p-0"
						ref={choicesRef}
					>
						<legend className="sr-only">
							{wait.type === "option" ? "Choose a reply" : "Choose a branch"}
						</legend>
						{wait.type === "option"
							? wait.node.options?.map((option, index) => (
									<button
										className="flex max-w-[90%] cursor-pointer items-start gap-2.5 rounded-2xl rounded-tr-sm border border-blue-500/32 bg-background px-3.5 py-2 text-start text-[0.9375rem] shadow-xs/5 outline-none transition-colors hover:bg-blue-500/8 focus-visible:ring-2 focus-visible:ring-blue-500"
										key={option.id}
										onClick={() => choose(index)}
										type="button"
									>
										<Kbd className="mt-0.5 shrink-0">{index + 1}</Kbd>
										<RichText value={option.text} />
									</button>
								))
							: wait.targets.map((target, index) => (
									<button
										className="flex max-w-[90%] cursor-pointer items-start gap-2.5 rounded-xl border border-dashed bg-background px-3.5 py-2 text-start text-muted-foreground text-sm outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
										key={target}
										onClick={() => choose(index)}
										type="button"
									>
										<Kbd className="mt-0.5 shrink-0">{index + 1}</Kbd>
										<span>
											<GitBranchIcon className="me-1 inline size-3.5" />
											{branchPreview(target, nodes)}
										</span>
									</button>
								))}
					</fieldset>
				) : null}
				<div className="flex items-center justify-between gap-2">
					<span
						className="text-muted-foreground text-xs tabular-nums"
						aria-live="polite"
					>
						{wait.type === "end"
							? "End of dialog"
							: `${visibleLines} / ${totalLines} lines`}
					</span>
					<div className="flex items-center gap-1.5">
						<Button
							aria-label="Previous line"
							disabled={steps.length <= 1}
							onClick={back}
							size="icon-sm"
							variant="ghost"
						>
							<Undo2Icon />
						</Button>
						{wait.type === "end" ? (
							<Button onClick={restart} size="sm" variant="outline">
								<RotateCcwIcon />
								Restart
							</Button>
						) : (
							<Button
								disabled={wait.type !== "next" && typing === -1}
								onClick={next}
								size="sm"
							>
								{typing !== -1 ? "Skip" : "Next"}
								<Kbd className="bg-primary-foreground/16 text-primary-foreground max-sm:hidden">
									Space
								</Kbd>
							</Button>
						)}
					</div>
				</div>
			</div>
			<div ref={endRef} />
		</div>
	);
}
