/** Recently opened files, kept per browser for the home page. */
import { useSyncExternalStore } from "react";
import { isRoot, type Root } from "@/lib/api";

export interface RecentEntry {
	root: Root;
	path: string;
	at: number;
}

const KEY = "fluorite.recent";
const LIMIT = 12;
const EMPTY: RecentEntry[] = [];

let cache: RecentEntry[] | null = null;
const listeners = new Set<() => void>();

function load(): RecentEntry[] {
	try {
		const parsed = JSON.parse(window.localStorage.getItem(KEY) ?? "[]");
		if (!Array.isArray(parsed)) return EMPTY;
		return parsed.filter(
			(e): e is RecentEntry =>
				typeof e === "object" &&
				e !== null &&
				typeof e.path === "string" &&
				typeof e.at === "number" &&
				typeof e.root === "string" &&
				isRoot(e.root),
		);
	} catch {
		return EMPTY;
	}
}

function snapshot(): RecentEntry[] {
	if (cache === null) cache = load();
	return cache;
}

export function pushRecent(root: Root, path: string): void {
	const next = [
		{ root, path, at: Date.now() },
		...snapshot().filter((e) => !(e.root === root && e.path === path)),
	].slice(0, LIMIT);
	cache = next;
	try {
		window.localStorage.setItem(KEY, JSON.stringify(next));
	} catch {
		// Best effort only.
	}
	for (const l of listeners) l();
}

export function clearRecent(): void {
	cache = EMPTY;
	try {
		window.localStorage.removeItem(KEY);
	} catch {
		// Best effort only.
	}
	for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
	listeners.add(listener);
	return () => listeners.delete(listener);
}

export function useRecent(): RecentEntry[] {
	return useSyncExternalStore(subscribe, snapshot, () => EMPTY);
}
