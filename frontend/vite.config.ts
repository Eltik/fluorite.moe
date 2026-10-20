import tailwindcss from "@tailwindcss/vite";
import { devtools } from "@tanstack/devtools-vite";

import { tanstackStart } from "@tanstack/react-start/plugin/vite";

import viteReact from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const config = defineConfig({
	resolve: { tsconfigPaths: true },
	plugins: [
		// Console piping echoes client logs through the dev server back to every client,
		// which turns any repeated warning into a feedback loop.
		devtools({ consolePiping: { enabled: false } }),
		tailwindcss(),
		tanstackStart(),
		viteReact(),
	],
});

export default config;
