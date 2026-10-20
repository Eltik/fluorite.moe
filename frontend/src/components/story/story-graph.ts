/**
 * Pure helpers for walking a flattened dialog graph (`/api/story/{id}`).
 *
 * Play mode is a list of steps (visited node ids). A step's node decides what
 * comes next: lines and transitions follow `next`, options wait for a choice,
 * and nodes with several real successors (conditions) wait for a branch pick.
 * Summary leaves (`transition/summary` with no `next`) are side notes shown
 * after the node that points at them, not steps of their own.
 */
import type { Dialog, DialogNode, Rich } from "@/lib/catalog";

export interface Step {
	id: string;
	/** Option index (option nodes) or branch index (multi-successor nodes). */
	choice?: number;
}

export type NodeMap = Map<string, DialogNode>;

export function nodeMap(dialog: Dialog): NodeMap {
	return new Map(dialog.nodes.map((n) => [n.id, n]));
}

export function isSummaryLeaf(node: DialogNode | undefined): boolean {
	return Boolean(
		node &&
			node.kind === "transition" &&
			node.action === "summary" &&
			node.next.length === 0,
	);
}

/** Successors that are part of the flow (summary leaves excluded). */
export function mainNext(node: DialogNode, nodes: NodeMap): string[] {
	const out = node.next.filter(
		(id) => nodes.has(id) && !isSummaryLeaf(nodes.get(id)),
	);
	// Several "finish" successors are equivalent endings; don't ask.
	if (out.length > 1 && out.every((id) => nodes.get(id)?.kind === "finish")) {
		return out.slice(0, 1);
	}
	return out;
}

export function sideSummaries(node: DialogNode, nodes: NodeMap): DialogNode[] {
	return node.next
		.map((id) => nodes.get(id))
		.filter((n): n is DialogNode => isSummaryLeaf(n));
}

/** Nodes that render nothing on their own in play mode. */
export function isSilent(node: DialogNode): boolean {
	return (
		node.kind === "transition" &&
		(node.action === "if" ||
			node.action === "branch" ||
			node.action === "empty" ||
			!node.action)
	);
}

export type Waiting =
	| { type: "option"; node: DialogNode }
	| { type: "branch"; node: DialogNode; targets: string[] }
	| { type: "end" }
	| { type: "next"; target: string };

/** What the last step waits for. */
export function waitingOn(steps: Step[], nodes: NodeMap): Waiting {
	const last = steps[steps.length - 1];
	const node = last ? nodes.get(last.id) : undefined;
	if (!node || node.kind === "finish") return { type: "end" };
	if (node.kind === "option") {
		if (last?.choice === undefined) return { type: "option", node };
		const option = node.options?.[last.choice];
		const target = option?.next ?? mainNext(node, nodes)[0];
		return target ? { type: "next", target } : { type: "end" };
	}
	const targets = mainNext(node, nodes);
	if (targets.length === 0) return { type: "end" };
	if (targets.length === 1)
		return { type: "next", target: targets[0] as string };
	if (last?.choice !== undefined && targets[last.choice]) {
		return { type: "next", target: targets[last.choice] as string };
	}
	return { type: "branch", node, targets };
}

const MAX_AUTO = 64;

/**
 * Appends the next step, then keeps going through silent / divider nodes so
 * every call reveals exactly one line, option, branch or ending.
 */
export function advance(steps: Step[], nodes: NodeMap): Step[] {
	let out = steps;
	for (let i = 0; i < MAX_AUTO; i++) {
		const wait = waitingOn(out, nodes);
		if (wait.type === "branch") {
			// Conditions can't be evaluated here: follow the first path; the
			// reader lets the viewer switch paths afterwards.
			const last = out[out.length - 1] as Step;
			out = [...out.slice(0, -1), { ...last, choice: 0 }];
			continue;
		}
		if (wait.type !== "next") return out;
		out = [...out, { id: wait.target }];
		const node = nodes.get(wait.target);
		if (!node || node.kind !== "transition") return out;
	}
	return out;
}

/** Initial steps: the start node, auto-advanced past silent nodes. */
export function initialSteps(dialog: Dialog, nodes: NodeMap): Step[] {
	const start = nodes.get(dialog.start) ?? dialog.nodes[0];
	if (!start) return [];
	const steps: Step[] = [{ id: start.id }];
	return start.kind === "transition" ? advance(steps, nodes) : steps;
}

/** Shortest path of steps from the start to `target` (deep links). */
export function pathTo(
	dialog: Dialog,
	nodes: NodeMap,
	target: string,
): Step[] | null {
	const start = nodes.get(dialog.start)?.id ?? dialog.nodes[0]?.id;
	if (!start || !nodes.has(target)) return null;
	const parent = new Map<string, { from: string; choice?: number }>();
	const queue = [start];
	const seen = new Set([start]);
	while (queue.length) {
		const id = queue.shift() as string;
		if (id === target) break;
		const node = nodes.get(id);
		if (!node) continue;
		const edges: { to: string; choice?: number }[] = [];
		if (node.kind === "option") {
			node.options?.forEach((o, i) => {
				const to = o.next ?? node.next[0];
				if (to) edges.push({ to, choice: i });
			});
		} else {
			const targets = mainNext(node, nodes);
			targets.forEach((to, i) => {
				edges.push({ to, choice: targets.length > 1 ? i : undefined });
			});
		}
		for (const edge of edges) {
			if (seen.has(edge.to)) continue;
			seen.add(edge.to);
			parent.set(edge.to, { from: id, choice: edge.choice });
			queue.push(edge.to);
		}
	}
	if (target !== start && !parent.has(target)) return null;
	const ids: string[] = [target];
	const choices = new Map<string, number | undefined>();
	let cur = target;
	while (cur !== start) {
		const p = parent.get(cur);
		if (!p) return null;
		choices.set(p.from, p.choice);
		ids.push(p.from);
		cur = p.from;
	}
	ids.reverse();
	return ids.map((id) => {
		const choice = choices.get(id);
		return choice === undefined ? { id } : { id, choice };
	});
}

/** First `n` characters of a rich value (typewriter reveal). */
export function sliceRich(rich: Rich, n: number): Rich {
	const out: Rich = [];
	let left = n;
	for (const seg of rich) {
		if (left <= 0) break;
		if (seg.text.length <= left) {
			out.push(seg);
			left -= seg.text.length || 0;
		} else {
			out.push({ ...seg, text: cutOutsideMarkup(seg.text, left) });
			left = 0;
		}
	}
	return out;
}

export function richLength(rich: Rich | null | undefined): number {
	return rich ? rich.reduce((n, s) => n + s.text.length, 0) : 0;
}

/** Cuts `text` at `n`, extending the cut past any furigana tag it would split. */
function cutOutsideMarkup(text: string, n: number): string {
	for (const match of text.matchAll(/<p="[^"]*"[^>]*>.*?<\/p>/g)) {
		const start = match.index ?? 0;
		const end = start + match[0].length;
		if (n > start && n < end) return text.slice(0, end);
	}
	return text.slice(0, n);
}
