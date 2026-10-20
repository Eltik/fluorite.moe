/**
 * Streamed byte fetching with progress, an in-memory promise cache that
 * de-duplicates in-flight requests, evicts failures, and keeps a bounded
 * number of finished buffers (LRU by insertion order).
 *
 * Consumers pass their own AbortSignal; the underlying request is aborted only
 * once every consumer has gone away.
 */

const BUDGET_BYTES = 320 * 1024 * 1024;

interface Entry {
	promise: Promise<ArrayBuffer>;
	loaded: number;
	total: number;
	done: boolean;
	refs: number;
	controller: AbortController;
	listeners: Set<(loaded: number, total: number) => void>;
}

const cache = new Map<string, Entry>();

function finishedBytes(): number {
	let sum = 0;
	for (const entry of cache.values()) if (entry.done) sum += entry.total;
	return sum;
}

function evict(): void {
	let over = finishedBytes() - BUDGET_BYTES;
	for (const [url, entry] of cache) {
		if (over <= 0) break;
		if (!entry.done) continue;
		cache.delete(url);
		over -= entry.total;
	}
}

async function download(url: string, entry: Entry): Promise<ArrayBuffer> {
	const res = await fetch(url, { signal: entry.controller.signal });
	if (!res.ok) {
		throw new Error(
			res.status === 404
				? `File not found: ${decodeURIComponent(url.split("/").pop() ?? url)}`
				: `Request failed (${res.status} ${res.statusText})`,
		);
	}
	const length = Number(res.headers.get("content-length") ?? 0);
	entry.total = Number.isFinite(length) ? length : 0;
	const emit = () => {
		for (const l of entry.listeners) l(entry.loaded, entry.total);
	};
	emit();
	if (!res.body) {
		const buf = await res.arrayBuffer();
		entry.loaded = entry.total = buf.byteLength;
		emit();
		return buf;
	}
	const reader = res.body.getReader();
	const chunks: Uint8Array[] = [];
	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		chunks.push(value);
		entry.loaded += value.byteLength;
		if (entry.loaded > entry.total) entry.total = entry.loaded;
		emit();
	}
	const out = new Uint8Array(entry.loaded);
	let offset = 0;
	for (const chunk of chunks) {
		out.set(chunk, offset);
		offset += chunk.byteLength;
	}
	entry.total = entry.loaded;
	emit();
	return out.buffer;
}

export interface FetchBytesOptions {
	signal?: AbortSignal;
	onProgress?: (loaded: number, total: number) => void;
}

export function fetchBytes(
	url: string,
	{ signal, onProgress }: FetchBytesOptions = {},
): Promise<ArrayBuffer> {
	let entry = cache.get(url);
	if (entry?.done) {
		// Refresh LRU position.
		cache.delete(url);
		cache.set(url, entry);
		onProgress?.(entry.total, entry.total);
		return entry.promise;
	}
	if (!entry) {
		const created: Entry = {
			promise: Promise.resolve(new ArrayBuffer(0)),
			loaded: 0,
			total: 0,
			done: false,
			refs: 0,
			controller: new AbortController(),
			listeners: new Set(),
		};
		created.promise = download(url, created).then(
			(buf) => {
				created.done = true;
				created.listeners.clear();
				evict();
				return buf;
			},
			(error: unknown) => {
				if (cache.get(url) === created) cache.delete(url);
				throw error;
			},
		);
		// Unhandled rejections are surfaced through the consumers below.
		created.promise.catch(() => {});
		cache.set(url, created);
		entry = created;
	}
	const active = entry;
	active.refs++;
	if (onProgress) {
		active.listeners.add(onProgress);
		onProgress(active.loaded, active.total);
	}
	return new Promise<ArrayBuffer>((resolve, reject) => {
		let settled = false;
		const release = () => {
			if (onProgress) active.listeners.delete(onProgress);
			active.refs--;
		};
		const onAbort = () => {
			if (settled) return;
			settled = true;
			release();
			if (active.refs <= 0 && !active.done) {
				active.controller.abort();
				if (cache.get(url) === active) cache.delete(url);
			}
			reject(new DOMException("Aborted", "AbortError"));
		};
		if (signal?.aborted) {
			onAbort();
			return;
		}
		signal?.addEventListener("abort", onAbort, { once: true });
		active.promise.then(
			(buf) => {
				if (settled) return;
				settled = true;
				signal?.removeEventListener("abort", onAbort);
				release();
				resolve(buf);
			},
			(error: unknown) => {
				if (settled) return;
				settled = true;
				signal?.removeEventListener("abort", onAbort);
				release();
				reject(error);
			},
		);
	});
}

export function isAbortError(error: unknown): boolean {
	return error instanceof DOMException && error.name === "AbortError";
}

/**
 * Aggregates byte progress over many files. `expect()` pre-registers a known
 * size (e.g. from a directory listing) so the total doesn't jump around.
 */
export class ProgressTracker {
	private files = new Map<string, { loaded: number; total: number }>();
	private onChange: (loaded: number, total: number) => void;
	private frame = 0;

	constructor(onChange: (loaded: number, total: number) => void) {
		this.onChange = onChange;
	}

	expect(key: string, size: number): void {
		const file = this.files.get(key);
		if (file) file.total = Math.max(file.total, size);
		else this.files.set(key, { loaded: 0, total: size });
		this.schedule();
	}

	reporter(key: string): (loaded: number, total: number) => void {
		this.expect(key, 0);
		return (loaded, total) => {
			const file = this.files.get(key);
			if (!file) return;
			file.loaded = loaded;
			file.total = Math.max(file.total, total, loaded);
			this.schedule();
		};
	}

	private schedule(): void {
		if (this.frame) return;
		this.frame = requestAnimationFrame(() => {
			this.frame = 0;
			let loaded = 0;
			let total = 0;
			for (const f of this.files.values()) {
				loaded += f.loaded;
				total += f.total;
			}
			this.onChange(loaded, total);
		});
	}

	dispose(): void {
		if (this.frame) cancelAnimationFrame(this.frame);
		this.frame = 0;
	}
}
