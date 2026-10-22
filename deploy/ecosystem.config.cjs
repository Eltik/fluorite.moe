/**
 * pm2 process definitions for fluorite.moe: the Rust API and the Bun SSR server of
 * the frontend. Both stay on the loopback; the reverse proxy (nginx or Caddy) is the
 * only thing that answers on a public port.
 *
 *   npm i -g pm2
 *   pm2 start deploy/ecosystem.config.cjs
 *   pm2 save && pm2 startup          # restart the processes after a reboot
 *   pm2 logs fluorite-api            # the log of the API
 *
 * The API reads its configuration from `backend/.env` itself, so the S3 settings live
 * there and not in this file. The variables below are the paths of the checkout;
 * change them if the checkout is somewhere else. Everything in `env` wins over
 * `backend/.env`.
 */
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const backend = path.join(root, "backend");
const frontend = path.join(root, "frontend");

module.exports = {
	apps: [
		{
			name: "fluorite-api",
			cwd: backend,
			// `cargo build --release --bin fluorite-api` puts it here.
			script: "./target/release/fluorite-api",
			exec_mode: "fork",
			instances: 1,
			autorestart: true,
			// The API is a single process; it uses every core for the search index.
			max_memory_restart: "1G",
			env: {
				FLUORITE_API_ADDR: "127.0.0.1:8787",
				FLUORITE_OUTPUT: path.join(backend, "output"),
				FLUORITE_CACHE_DIR: path.join(backend, "output", ".cache"),
				FLUORITE_STATE: path.join(backend, "data", "update-state.json"),
				RUST_LOG: "info",
			},
		},
		{
			name: "fluorite-web",
			cwd: frontend,
			// `bun install --frozen-lockfile && bun run build` puts it here.
			script: ".output/server/index.mjs",
			interpreter: "bun",
			exec_mode: "fork",
			instances: 1,
			autorestart: true,
			max_memory_restart: "1G",
			env: {
				NODE_ENV: "production",
				HOST: "127.0.0.1",
				PORT: "3000",
				// Where the SSR server reaches the API. It also proxies /api,
				// /files, /thumb and /audio/* to it, so the site is one origin.
				FLUORITE_API_URL: "http://127.0.0.1:8787",
				// Empty: the browser uses the origin of the page (the proxy).
				FLUORITE_PUBLIC_API_URL: "",
			},
		},
	],
};
