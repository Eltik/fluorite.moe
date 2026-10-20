import type * as React from "react";
import { useSyncExternalStore } from "react";
import { formatDateTime, formatRelative } from "@/lib/format";

const MINUTE = 60_000;

function subscribe(onTick: () => void): () => void {
	const id = window.setInterval(onTick, MINUTE);
	return () => window.clearInterval(id);
}

/**
 * "6 hours ago", hydration-safe: the server (and the hydration pass) render
 * the absolute UTC date, the client swaps in the relative form afterwards and
 * refreshes it every minute. Server and browser clocks never have to agree.
 */
export function RelativeTime({
	iso,
	className,
}: {
	iso: string;
	className?: string;
}): React.ReactElement {
	const absolute = formatDateTime(iso);
	const text = useSyncExternalStore(
		subscribe,
		() => formatRelative(iso),
		() => absolute,
	);
	return (
		<time className={className} dateTime={iso} title={absolute}>
			{text}
		</time>
	);
}
