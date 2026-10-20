# fluorite frontend

The web app for fluorite.moe: a data studio for Arknights: Endfield over the
`fluorite-api` HTTP API. Built with TanStack Start (SSR, file routes), React 19,
TanStack Query, Tailwind v4 and COSS UI (Base UI) components, run with Bun.

See the root [README](../README.md#web-app) for how the API, the update pipeline and
this app fit together in production.

## Scripts

| Command | What it does |
|---|---|
| `bun install` | Install dependencies |
| `bun run dev` | Vite dev server with HMR on `:3000` (SSR included) |
| `bun run build` | Production build into `.output/` (Nitro, `bun` preset) |
| `bun run start` | Serve the build: `bun .output/server/index.mjs` (needs Bun >= 1.3.14) |
| `bun run check` | Biome lint + format check (`bun run format` / `lint` individually) |
| `bunx tsc --noEmit` | Type-check |
| `bun run generate-routes` | Regenerate `src/routeTree.gen.ts` (the dev server and build do this too) |
| `bun dev/mock-api.ts` | Stand-in API over the output directory, for when the Rust API isn't built |

`../scripts/dev.sh` starts the API and the dev server together; `../scripts/serve.sh`
builds what is stale and runs both in production mode.

## Configuration

The API location is read from the server's environment at startup, so one build
works for any deployment. The root route renders it into the page
(`window.__FLUORITE_API_URL__`), and `src/lib/api.ts` uses that in the browser.

| Variable | Default | Meaning |
|---|---|---|
| `PORT` / `HOST` | `3000` / all interfaces | Listen address of `bun run start` (also `NITRO_PORT` / `NITRO_HOST`) |
| `FLUORITE_API_URL` | `http://127.0.0.1:8787` | Where the server reaches the API: SSR data fetches and the proxy |
| `FLUORITE_PUBLIC_API_URL` | same origin in production, `FLUORITE_API_URL` in dev | API base the browser uses. Empty means "this origin" |
| `FLUORITE_PROXY` | on | `0` turns off the same-origin proxy |
| `VITE_API_URL` | unset | Build-time fallback for both API URLs (old dev setups) |

**Same-origin proxy.** `src/server.ts` forwards `/api/*`, `/files/*`, `/thumb/*` and
`/audio/{pack}/{file}` to `FLUORITE_API_URL`, streaming bodies and passing `Range`,
`If-None-Match`, 206/304 and compressed bodies through. The `/audio` page itself is
an app route. With the proxy, production needs only one public origin and no CORS.
To serve the API from its own origin instead, set `FLUORITE_PUBLIC_API_URL` to it
(and `FLUORITE_PROXY=0`).

**Headers.** `src/server.ts` adds a CSP (production only, since Vite's dev client
needs inline modules and a websocket), `X-Frame-Options: DENY`, `nosniff`,
`Referrer-Policy`, `Cross-Origin-Opener-Policy` and `Permissions-Policy` to app
responses. The CSP allows the public API origin for `img-src`, `media-src` and
`connect-src`, `blob:`/`data:` for textures that three.js unpacks from GLBs, and
`'wasm-unsafe-eval'` for the meshopt decoder. Hashed bundles under `/assets/` are
served with `Cache-Control: public, max-age=31536000, immutable` (see
`vite.config.ts`). HTML is not cached.

## Structure

```
src/
  routes/          file routes (TanStack Router); routeTree.gen.ts is generated
  components/
    ui/            COSS UI primitives (Base UI + Tailwind), added via the @coss registry
    shell/         app shell: sidebar, top bar + breadcrumbs, command palette, API status
    catalog/       entity pages (operators, enemies, weapons, items, ...), filters, charts
    studio/        3D model studio (three.js / r3f): stage, loader, animation, shading
    story/         dialogue reader and graph
    audio/         audio engine, player bar, entry list, voice lines
    gallery/ browser/ table/ viewers/   file explorers and per-type viewers (image, GLB, JSON, Lua)
    home/ rich-text/                    home page sections, game rich-text markup
  lib/
    api.ts         typed API client, URL builders, runtime API configuration
    queries.ts     query option factories (one per endpoint) shared by loaders and components
    catalog.ts     catalog + story contract types and queries
    audio.ts       audio endpoints and clip URLs
    json-parse.ts  int64-safe JSON.parse
    settings.ts    per-viewer settings (theme, text language, density) in localStorage
    ...            formatting, navigation, studio links, route metadata
  server.ts        server entry: same-origin proxy + security headers
  router.tsx       router + QueryClient setup, SSR query integration
dev/mock-api.ts    dev-only API stand-in
```

### Routes

| Path | Page |
|---|---|
| `/` | Home: status, featured operators, recent files |
| `/catalog`, `/catalog/$kind`, `/catalog/$kind/$id` | Entity catalog: kinds, filtered grid, detail page |
| `/studio`, `/studio/$kind/$id` | Full-bleed 3D model viewer |
| `/story`, `/story/$dialogId` | Story dialogue index and reader |
| `/audio` | Audio packages, entries and voice lines |
| `/tables`, `/tables/$name` | Game tables as a data grid or JSON |
| `/textures` | Texture gallery |
| `/browse/$root` | File explorer over an output root (`textures`, `meshes`, `gamedata`, `jsondata`, `lua`, ...) |

## Conventions

- **URL state.** Anything worth sharing or restoring (paths, filters, tabs, selected
  clip, gender, ...) lives in search params, declared per route with a zod
  `validateSearch` schema; fields use `.catch()` where a bad value should fall back to
  the default instead of erroring. Per-viewer preferences go to `lib/settings.ts`.
- **Queries.** Every endpoint has one factory in `lib/queries.ts` (or `lib/catalog.ts` /
  `lib/audio.ts`) returning `queryOptions`. Loaders prefetch with it
  (`context.queryClient.ensureQueryData(...)` / `.query(...)`) and components read
  with `useQuery` / `useSuspenseQuery` and the same factory, so keys always match and
  SSR data hydrates without refetching. Keys are arrays that start with the resource
  name and include every input (`["tree", root, path]`,
  `["catalog", kind, lang, "detail", id, gender]`, ...).
- **API access.** Only through `lib/api.ts`: `api.*` methods, `getJson`, and the URL
  builders (`fileUrl`, `thumbUrl`, `modelFileUrl`). Never concatenate the API base by
  hand; it can be empty (same origin). Errors surface as `ApiError` with `status` /
  `code` (`status === 0` means unreachable, `501` means not mapped yet), and
  `components/status-views.tsx` renders them.
- **int64-safe JSON.** Game tables carry signed 64-bit ids that `JSON.parse` would
  round. Responses are parsed with `parseJsonSafe` (`lib/json-parse.ts`), which
  turns unsafe integer literals into strings, so ids round-trip exactly (e.g. to
  `/api/i18n/{lang}/resolve`).
- **UI components.** Use the COSS UI primitives in `components/ui/` (add more with
  `bunx shadcn@latest add @coss/<name>`), Tailwind tokens from `styles.css`, and
  `lucide-react` icons. Feature components compose these instead of styling raw
  elements.
- **Heavy code stays lazy.** three.js / r3f, Shiki and the devtools are imported
  dynamically so they only load on the pages that use them.
- **Formatting.** Biome (tabs, organized imports). Run `bun run check` before committing.
