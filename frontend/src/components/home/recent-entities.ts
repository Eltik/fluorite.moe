/**
 * Recently viewed catalog pages (operators, items, …), kept per browser next
 * to the file history in `lib/recent.ts`. Fed by the app shell, read by home.
 */
import { useSyncExternalStore } from "react";
import { CATALOG_KINDS, type CatalogKind } from "@/lib/api";

export interface RecentEntity {
	kind: CatalogKind;
	id: string;
	name: string;
	at: number;
}

const KEY = "fluorite.recent-entities";
const LIMIT = 12;
const EMPTY: RecentEntity[] = [];

let cache: RecentEntity[] | null = null;
const listeners = new Set<() => void>();

function load(): RecentEntity[] {
	try {
		const parsed = JSON.parse(window.localStorage.getItem(KEY) ?? "[]");
		if (!Array.isArray(parsed)) return EMPTY;
		return parsed.filter(
			(e): e is RecentEntity =>
				typeof e === "object" &&
				e !== null &&
				typeof e.id === "string" &&
				typeof e.name === "string" &&
				typeof e.at === "number" &&
				(CATALOG_KINDS as readonly string[]).includes(e.kind),
		);
	} catch {
		return EMPTY;
	}
}

function snapshot(): RecentEntity[] {
	if (cache === null) cache = load();
	return cache;
}

function emit(next: RecentEntity[]): void {
	cache = next;
	try {
		if (next.length) window.localStorage.setItem(KEY, JSON.stringify(next));
		else window.localStorage.removeItem(KEY);
	} catch {
		// Best effort only.
	}
	for (const l of listeners) l();
}

export function pushRecentEntity(kind: CatalogKind, id: string, name: string) {
	const current = snapshot();
	if (current[0]?.kind === kind && current[0]?.id === id) {
		if (current[0].name === name) return;
	}
	emit(
		[
			{ kind, id, name, at: Date.now() },
			...current.filter((e) => !(e.kind === kind && e.id === id)),
		].slice(0, LIMIT),
	);
}

export function clearRecentEntities(): void {
	emit(EMPTY);
}

function subscribe(listener: () => void): () => void {
	listeners.add(listener);
	return () => listeners.delete(listener);
}

export function useRecentEntities(): RecentEntity[] {
	return useSyncExternalStore(subscribe, snapshot, () => EMPTY);
}
