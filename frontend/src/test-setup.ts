// `bun test` preload. `lib/api.ts` reads its API base from `window` at import
// time (the browser branch, since Vite's `import.meta.env.SSR` is unset here).
(globalThis as { window?: unknown }).window = {
	__FLUORITE_API_URL__: "https://api.test/",
};
