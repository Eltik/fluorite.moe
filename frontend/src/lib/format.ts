const numberFormat = new Intl.NumberFormat("en-US");

export function formatNumber(value: number): string {
	return numberFormat.format(value);
}

export function formatBytes(bytes: number | undefined): string {
	if (bytes === undefined || Number.isNaN(bytes)) return "—";
	if (bytes < 1024) return `${bytes} B`;
	const units = ["KB", "MB", "GB", "TB"];
	let value = bytes / 1024;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

const relative = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

export function formatRelative(iso: string, now: number = Date.now()): string {
	const then = Date.parse(iso);
	if (Number.isNaN(then)) return iso;
	const seconds = Math.round((then - now) / 1000);
	const steps: [Intl.RelativeTimeFormatUnit, number][] = [
		["year", 31_536_000],
		["month", 2_592_000],
		["week", 604_800],
		["day", 86_400],
		["hour", 3_600],
		["minute", 60],
	];
	for (const [unit, size] of steps) {
		if (Math.abs(seconds) >= size) {
			return relative.format(Math.round(seconds / size), unit);
		}
	}
	return "just now";
}

const MONTHS = [
	"Jan",
	"Feb",
	"Mar",
	"Apr",
	"May",
	"Jun",
	"Jul",
	"Aug",
	"Sep",
	"Oct",
	"Nov",
	"Dec",
];

/**
 * `Oct 20, 2026, 08:35 UTC`. Built by hand from UTC fields so the server
 * (Node ICU) and the browser render identical text: `toLocaleString` differs
 * between ICU builds ("at" vs ","), which breaks hydration.
 */
export function formatDateTime(iso: string): string {
	const date = new Date(iso);
	if (Number.isNaN(date.getTime())) return iso;
	const pad = (n: number) => String(n).padStart(2, "0");
	return `${MONTHS[date.getUTCMonth()]} ${date.getUTCDate()}, ${date.getUTCFullYear()}, ${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())} UTC`;
}

/** `Oct 20, 2026` (UTC, deterministic across server and client). */
export function formatDate(iso: string): string {
	const date = new Date(iso);
	if (Number.isNaN(date.getTime())) return iso;
	return `${MONTHS[date.getUTCMonth()]} ${date.getUTCDate()}, ${date.getUTCFullYear()}`;
}

/** `2.1M`, `73k`, `1.5k`, `340`: short counts for badges and summaries. */
export function formatCompact(n: number): string {
	if (n >= 1_000_000)
		return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
	if (n >= 10_000) return `${Math.round(n / 1000)}k`;
	if (n >= 1_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
	return String(n);
}

export function extname(name: string): string {
	const dot = name.lastIndexOf(".");
	return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export type PreviewKind = "json" | "code" | "image" | "model" | "binary";

const TEXT_EXTENSIONS = new Set([
	"lua",
	"txt",
	"xml",
	"csv",
	"yaml",
	"yml",
	"ini",
	"shader",
	"cs",
	"md",
	"log",
]);

export function previewKind(name: string): PreviewKind {
	const ext = extname(name);
	if (ext === "json") return "json";
	if (ext === "png" || ext === "jpg" || ext === "jpeg" || ext === "webp")
		return "image";
	if (ext === "glb" || ext === "gltf") return "model";
	if (TEXT_EXTENSIONS.has(ext)) return "code";
	return "binary";
}

export function dirname(path: string): string {
	const slash = path.lastIndexOf("/");
	return slash === -1 ? "" : path.slice(0, slash);
}

export function basename(path: string): string {
	const slash = path.lastIndexOf("/");
	return slash === -1 ? path : path.slice(slash + 1);
}
