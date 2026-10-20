import tailwindcss from "@tailwindcss/vite";
import { devtools } from "@tanstack/devtools-vite";

import { tanstackStart } from "@tanstack/react-start/plugin/vite";

import viteReact from "@vitejs/plugin-react";
import { nitro } from "nitro/vite";
import { defineConfig } from "vite";

const config = defineConfig({
	resolve: { tsconfigPaths: true },
	plugins: [
		// Console piping echoes client logs through the dev server back to every client,
		// which turns any repeated warning into a feedback loop.
		devtools({ consolePiping: { enabled: false } }),
		tailwindcss(),
		tanstackStart(),
		// Production server: `vite build` emits `.output/server/index.mjs` for Bun.
		// Hashed bundles under /assets are immutable; everything else revalidates.
		nitro({
			preset: "bun",
			routeRules: {
				"/assets/**": {
					headers: { "cache-control": "public, max-age=31536000, immutable" },
				},
				"/favicon.svg": {
					headers: { "cache-control": "public, max-age=86400" },
				},
			},
		}),
		viteReact(),
	],
});

export default config;
