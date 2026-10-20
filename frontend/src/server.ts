/**
 * Server entry: the TanStack Start handler plus two production concerns.
 *
 * 1. Same-origin proxy. `/api/*`, `/files/*`, `/thumb/*` and `/audio/{pack}/{file}`
 *    are forwarded to `FLUORITE_API_URL`, so a deployment can expose one public
 *    origin. Bodies stream in both directions; `Range`, `If-None-Match` and the
 *    upstream status (206, 304, ...) pass through untouched. Set
 *    `FLUORITE_PROXY=0` to disable it when the API is published on its own origin.
 * 2. Security headers on every page response, with a CSP whose fetch / image /
 *    media sources include the public API origin.
 */
import handler, { createServerEntry } from "@tanstack/react-start/server-entry";
import { publicApiUrl, upstreamApiUrl } from "@/lib/api";

type Env = Record<string, string | undefined>;
const env: Env =
	(globalThis as { process?: { env: Env } }).process?.env ?? ({} as Env);

const PROXY_ENABLED = env.FLUORITE_PROXY !== "0";
const PROXY_PREFIXES = ["/api/", "/files/", "/thumb/"];
/** `/audio` itself is an app page; only `/audio/{pack}/{file}` clips are API. */
const AUDIO_CLIP = /^\/audio\/[^/]+\/[^/]+$/;

function isProxied(pathname: string): boolean {
	return (
		PROXY_PREFIXES.some((p) => pathname.startsWith(p)) ||
		AUDIO_CLIP.test(pathname)
	);
}

/** Connection-scoped headers that must not be forwarded (RFC 9110 §7.6.1). */
const HOP_BY_HOP = [
	"connection",
	"keep-alive",
	"proxy-authenticate",
	"proxy-authorization",
	"proxy-connection",
	"te",
	"trailer",
	"transfer-encoding",
	"upgrade",
];

/** Bun's fetch can hand back the raw (still-compressed) body; Node's cannot. */
const IS_BUN = typeof (globalThis as { Bun?: unknown }).Bun !== "undefined";

async function proxy(request: Request, url: URL): Promise<Response> {
	const target = `${upstreamApiUrl()}${url.pathname}${url.search}`;
	const headers = new Headers(request.headers);
	for (const h of [...HOP_BY_HOP, "host"]) headers.delete(h);
	headers.set("x-forwarded-host", url.host);
	headers.set("x-forwarded-proto", url.protocol.replace(":", ""));
	if (!IS_BUN) headers.delete("accept-encoding");

	const hasBody = request.method !== "GET" && request.method !== "HEAD";
	const init: RequestInit & { duplex?: "half"; decompress?: boolean } = {
		method: request.method,
		headers,
		body: hasBody ? request.body : undefined,
		duplex: hasBody ? "half" : undefined,
		redirect: "manual",
		signal: request.signal,
		decompress: false,
	};

	let upstream: Response;
	try {
		upstream = await fetch(target, init);
	} catch {
		return Response.json(
			{
				error: {
					code: "upstream_unreachable",
					message: "The fluorite API is not reachable from the web server.",
				},
			},
			{ status: 502, headers: { "cache-control": "no-store" } },
		);
	}

	const out = new Headers(upstream.headers);
	for (const h of HOP_BY_HOP) out.delete(h);
	if (!IS_BUN) {
		// Node decoded the body already; the original framing no longer applies.
		out.delete("content-encoding");
		out.delete("content-length");
	}
	return new Response(upstream.body, {
		status: upstream.status,
		statusText: upstream.statusText,
		headers: out,
	});
}

function originOf(base: string): string | null {
	try {
		return base ? new URL(base).origin : null;
	} catch {
		return null;
	}
}

function contentSecurityPolicy(): string {
	const api = originOf(publicApiUrl());
	const withApi = (...sources: string[]) =>
		[...sources, ...(api ? [api] : [])].join(" ");
	return [
		"default-src 'self'",
		// Inline: the pre-hydration theme script, the API config and Start's
		// hydration payload. wasm: three.js' meshopt decoder.
		"script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'",
		"style-src 'self' 'unsafe-inline'",
		// blob:/data: cover textures GLTFLoader unpacks from GLBs and model exports.
		`img-src ${withApi("'self'", "data:", "blob:")}`,
		`media-src ${withApi("'self'", "blob:")}`,
		`connect-src ${withApi("'self'", "blob:", "data:")}`,
		"font-src 'self' data:",
		"worker-src 'self' blob:",
		"object-src 'none'",
		"base-uri 'self'",
		"form-action 'self'",
		"frame-ancestors 'none'",
	].join("; ");
}

const CSP = contentSecurityPolicy();

function withSecurityHeaders(response: Response): Response {
	const set = (name: string, value: string) => {
		if (!response.headers.has(name)) response.headers.set(name, value);
	};
	try {
		// Vite's dev client needs inline modules and a websocket; CSP is prod-only.
		if (import.meta.env.PROD) set("content-security-policy", CSP);
		set("x-content-type-options", "nosniff");
		set("x-frame-options", "DENY");
		set("referrer-policy", "strict-origin-when-cross-origin");
		set("cross-origin-opener-policy", "same-origin");
		set(
			"permissions-policy",
			"camera=(), microphone=(), geolocation=(), payment=(), usb=()",
		);
		return response;
	} catch {
		// Immutable headers (e.g. a Response passed through from fetch): copy once.
		return withSecurityHeaders(
			new Response(response.body, {
				status: response.status,
				statusText: response.statusText,
				headers: new Headers(response.headers),
			}),
		);
	}
}

export default createServerEntry({
	async fetch(request, opts) {
		const url = new URL(request.url);
		if (PROXY_ENABLED && isProxied(url.pathname)) return proxy(request, url);
		return withSecurityHeaders(await handler.fetch(request, opts));
	},
});
