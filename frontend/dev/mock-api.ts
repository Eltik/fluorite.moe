/**
 * Dev-only stand-in for `fluorite-api` that implements the v1 contract over the
 * real extraction output on disk. Never imported by the app; run it manually
 * while the Rust API is unavailable:
 *
 *   bun dev/mock-api.ts            # serves http://127.0.0.1:8787
 *   FLUORITE_OUTPUT=... PORT=... bun dev/mock-api.ts
 *
 * Thumbnails are served as the original PNG and catalogs answer 501.
 */
import { createReadStream, readdirSync, readFileSync, statSync } from "node:fs";
import { createServer, type ServerResponse } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";

const OUTPUT = resolve(
	process.env.FLUORITE_OUTPUT ??
		join(import.meta.dirname, "../../backend/output"),
);
const PORT = Number(process.env.PORT ?? 8787);

const ROOTS: Record<string, string> = {
	textures: "textures",
	meshes: "meshes",
	gamedata: "gamedata",
	tables: "decrypted/Table/TableCfg",
	jsondata: "decrypted/JsonData/Json",
	lua: "decrypted/Lua/LuaScripts",
	audio: "decrypted/Audio",
};

const hidden = (name: string) =>
	name.startsWith(".") ||
	name.endsWith(".tmp") ||
	name.endsWith(".mpk") ||
	name === "blc_metadata.txt";

type IndexItem = { root: string; path: string; name: string; size: number };
const index: IndexItem[] = [];

function walk(root: string, base: string, rel: string) {
	let entries: import("node:fs").Dirent[];
	try {
		entries = readdirSync(join(base, rel), { withFileTypes: true });
	} catch {
		return;
	}
	for (const e of entries) {
		if (hidden(e.name)) continue;
		const p = rel ? `${rel}/${e.name}` : e.name;
		if (e.isDirectory()) walk(root, base, p);
		else if (root !== "tables" || e.name.endsWith(".json")) {
			index.push({
				root,
				path: p,
				name: e.name,
				size: statSync(join(base, p)).size,
			});
		}
	}
}

const t0 = Date.now();
for (const [root, dir] of Object.entries(ROOTS))
	walk(root, join(OUTPUT, dir), "");
console.log(`indexed ${index.length} files in ${Date.now() - t0}ms`);

function safe(root: string, rel: string): string | null {
	const dir = ROOTS[root];
	if (!dir) return null;
	const base = resolve(OUTPUT, dir);
	const full = normalize(join(base, rel));
	if (full !== base && !full.startsWith(base + sep)) return null;
	return full;
}

function json(res: ServerResponse, status: number, body: unknown) {
	res.writeHead(status, {
		"content-type": "application/json",
		"access-control-allow-origin": "*",
	});
	res.end(JSON.stringify(body));
}

function err(res: ServerResponse, status: number, code: string, msg: string) {
	json(res, status, { error: { code, message: msg } });
}

const MIME: Record<string, string> = {
	".png": "image/png",
	".glb": "model/gltf-binary",
	".json": "application/json",
	".lua": "text/plain; charset=utf-8",
	".txt": "text/plain; charset=utf-8",
};

function sendFile(res: ServerResponse, full: string) {
	try {
		const st = statSync(full);
		if (!st.isFile()) return err(res, 404, "not_found", "not a file");
		res.writeHead(200, {
			"content-type": MIME[extname(full)] ?? "application/octet-stream",
			"content-length": st.size,
			"access-control-allow-origin": "*",
			"cache-control": "public, max-age=3600",
		});
		createReadStream(full).pipe(res);
	} catch {
		err(res, 404, "not_found", "file not found");
	}
}

const i18nCache = new Map<string, Record<string, string>>();
function i18n(lang: string): Record<string, string> | null {
	const hit = i18nCache.get(lang);
	if (hit) return hit;
	try {
		const raw = readFileSync(
			join(OUTPUT, ROOTS.tables, `I18nTextTable_${lang}.json`),
			"utf8",
		);
		const map = JSON.parse(raw) as Record<string, string>;
		i18nCache.set(lang, map);
		return map;
	} catch {
		return null;
	}
}

function readUpdate() {
	try {
		const lines = readFileSync(join(OUTPUT, "update-log.jsonl"), "utf8")
			.trim()
			.split("\n");
		return JSON.parse(lines[lines.length - 1] ?? "{}") as {
			at?: string;
			version?: string;
			res_version?: string;
		};
	} catch {
		return {};
	}
}

createServer((req, res) => {
	const url = new URL(req.url ?? "/", `http://${req.headers.host}`);
	const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
	if (req.method === "OPTIONS") {
		res.writeHead(204, {
			"access-control-allow-origin": "*",
			"access-control-allow-headers": "content-type",
			"access-control-allow-methods": "GET, POST",
		});
		return res.end();
	}
	const q = url.searchParams;

	if (parts[0] === "files" && parts[1]) {
		const full = safe(parts[1], parts.slice(2).join("/"));
		if (!full) return err(res, 400, "bad_request", "bad path");
		return sendFile(res, full);
	}
	if (parts[0] === "thumb") {
		const full = safe("textures", parts.slice(1).join("/"));
		if (!full) return err(res, 400, "bad_request", "bad path");
		return sendFile(res, full);
	}
	if (parts[0] !== "api") return err(res, 404, "not_found", "no route");

	const [, ep, a, b] = parts;
	if (ep === "status") {
		const u = readUpdate();
		const counts: Record<string, number> = {};
		for (const it of index) counts[it.root] = (counts[it.root] ?? 0) + 1;
		return json(res, 200, {
			version: u.version ?? "unknown",
			resVersion: u.res_version ?? "unknown",
			updatedAt: u.at ?? new Date().toISOString(),
			schemaVersion: 1,
			counts: {
				textures: counts.textures ?? 0,
				meshes: counts.meshes ?? 0,
				tables: counts.tables ?? 0,
				jsondata: counts.jsondata ?? 0,
				lua: counts.lua ?? 0,
				gamedata: counts.gamedata ?? 0,
			},
		});
	}
	if (ep === "tree" && a) {
		const rel = q.get("path") ?? "";
		const full = safe(a, rel);
		if (!full) return err(res, 400, "bad_request", "bad path");
		let dirents: import("node:fs").Dirent[];
		try {
			dirents = readdirSync(full, { withFileTypes: true });
		} catch {
			return err(res, 404, "not_found", `no such directory: ${rel}`);
		}
		const entries = dirents
			.filter((d) => !hidden(d.name))
			.map((d) => {
				const p = rel ? `${rel}/${d.name}` : d.name;
				if (d.isDirectory()) {
					const children = readdirSync(join(full, d.name)).filter(
						(n) => !hidden(n),
					).length;
					return { name: d.name, path: p, kind: "dir" as const, children };
				}
				return {
					name: d.name,
					path: p,
					kind: "file" as const,
					size: statSync(join(full, d.name)).size,
				};
			})
			.sort((x, y) =>
				x.kind !== y.kind
					? x.kind === "dir"
						? -1
						: 1
					: x.name.localeCompare(y.name),
			);
		const offset = Number(q.get("offset") ?? 0);
		const limit = Number(q.get("limit") ?? 200);
		return json(res, 200, {
			root: a,
			path: rel,
			entries: entries.slice(offset, offset + limit),
			total: entries.length,
			offset,
			limit,
		});
	}
	if (ep === "search") {
		const tokens = (q.get("q") ?? "")
			.toLowerCase()
			.split(/\s+/)
			.filter(Boolean);
		const root = q.get("root");
		const ext = q.get("ext");
		const limit = Number(q.get("limit") ?? 50);
		const offset = Number(q.get("offset") ?? 0);
		const hits = tokens.length
			? index.filter((it) => {
					if (root && it.root !== root) return false;
					if (ext && !it.name.endsWith(`.${ext}`)) return false;
					const p = it.path.toLowerCase();
					return tokens.every((t) => p.includes(t));
				})
			: [];
		return json(res, 200, {
			query: q.get("q") ?? "",
			total: hits.length,
			items: hits.slice(offset, offset + limit),
		});
	}
	if (ep === "tables" && !a) {
		return json(res, 200, {
			tables: index
				.filter((it) => it.root === "tables")
				.map((it) => ({ name: it.name.replace(/\.json$/, ""), size: it.size })),
		});
	}
	if (ep === "tables" && a) {
		const full = safe("tables", `${a.replace(/\.json$/, "")}.json`);
		if (!full) return err(res, 400, "bad_request", "bad name");
		return sendFile(res, full);
	}
	if (ep === "i18n" && a === "languages") {
		return json(res, 200, {
			languages: index
				.filter(
					(it) => it.root === "tables" && it.name.startsWith("I18nTextTable_"),
				)
				.map((it) => it.name.slice(14, -5)),
		});
	}
	if (ep === "i18n" && a && b === "resolve" && req.method === "POST") {
		const map = i18n(a);
		if (!map) return err(res, 404, "not_found", `unknown language ${a}`);
		let body = "";
		req.on("data", (c: Buffer) => {
			body += c.toString();
		});
		req.on("end", () => {
			try {
				const { keys } = JSON.parse(body) as { keys: string[] };
				const texts: Record<string, string> = {};
				for (const k of keys) if (k in map) texts[k] = map[k] ?? "";
				json(res, 200, { texts });
			} catch {
				err(res, 400, "bad_request", "invalid body");
			}
		});
		return;
	}
	if (ep === "i18n" && a && b) {
		const map = i18n(a);
		if (!map) return err(res, 404, "not_found", `unknown language ${a}`);
		if (!(b in map)) return err(res, 404, "not_found", "unknown key");
		return json(res, 200, { key: b, text: map[b] });
	}
	if (ep === "catalog") {
		return err(res, 501, "not_implemented", "catalogs are not mapped yet");
	}
	return err(res, 404, "not_found", "no route");
}).listen(PORT, "127.0.0.1", () => {
	console.log(
		`mock fluorite-api on http://127.0.0.1:${PORT} (data: ${OUTPUT})`,
	);
});
