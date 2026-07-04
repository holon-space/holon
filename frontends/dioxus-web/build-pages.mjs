#!/usr/bin/env node
// Static-assembly for the GitHub Pages deploy of the holon-dioxus-web app.
//
// The dev server (serve.mjs) serves the worker wasm + its ESM deps with
// COOP/COEP headers and rewrites bare specifiers on the fly. GitHub Pages can
// do neither, so this script BAKES both into a flat static tree:
//
//   site/
//   ├── index.html                    ← Trunk-built Dioxus app (from dist/)
//   ├── coi-serviceworker.js          ← patches COOP/COEP client-side
//   ├── holon_worker.wasm32-wasi.wasm ← worker (absolute root URL)
//   ├── <trunk wasm/js/snippets>
//   ├── web/                          ← worker-entry.mjs + OPFS stubs
//   │     (bare imports rewritten to /node_modules/...)
//   └── node_modules/                 ← the 5 ESM deps the worker imports
//
// Usage (after `trunk build --release` + `napi build` have run):
//   node build-pages.mjs --out <site-dir> --worker-wasm <path-to-holon_worker.wasm>
//
// Prerequisites that must have run first:
//   1. `trunk build --release` in frontends/dioxus-web  → dist/
//   2. `napi build --features browser --profile release-official --platform
//        --target wasm32-wasip1-threads --no-js` in frontends/holon-worker
//      → target/wasm32-wasip1-threads/release-official/holon_worker.wasm

import {
	mkdirSync,
	existsSync,
	readFileSync,
	writeFileSync,
	cpSync,
	readdirSync,
	statSync,
} from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const WORKER_ROOT = join(HERE, "../holon-worker");
const DIST = join(HERE, "dist");

// Bare specifiers the worker's ESM graph uses (mirrors serve.mjs). Every one
// resolves to a static /node_modules/... path so no bundler is needed.
const BARE_IMPORTS = {
	"@napi-rs/wasm-runtime": "/node_modules/@napi-rs/wasm-runtime/runtime.js",
	"@emnapi/runtime": "/node_modules/@emnapi/runtime/dist/emnapi.mjs",
	"@emnapi/core": "/node_modules/@emnapi/core/dist/emnapi-core.mjs",
	"@emnapi/wasi-threads":
		"/node_modules/@emnapi/wasi-threads/dist/wasi-threads.mjs",
	"@tybys/wasm-util": "/node_modules/@tybys/wasm-util/lib/mjs/index.mjs",
};

// The five npm packages that back those bare specifiers. Copied wholesale so
// their relative sub-imports (e.g. wasm-runtime/runtime.js → ./fs-proxy.js)
// keep resolving.
const NODE_DEPS = [
	"@napi-rs/wasm-runtime",
	"@emnapi/runtime",
	"@emnapi/core",
	"@emnapi/wasi-threads",
	"@tybys/wasm-util",
];

// coi-serviceworker, pinned to a released npm version (served same-origin, not
// from a CDN, because a service worker must be same-origin).
const COI_URL =
	"https://cdn.jsdelivr.net/npm/coi-serviceworker@0.1.7/coi-serviceworker.min.js";

function parseArgs(argv) {
	const out = {};
	for (let i = 0; i < argv.length; i++) {
		if (argv[i] === "--out" && argv[i + 1]) out.out = argv[++i];
		else if (argv[i] === "--worker-wasm" && argv[i + 1])
			out.workerWasm = argv[++i];
		else if (argv[i] === "--coi" && argv[i + 1]) out.coi = argv[++i];
	}
	if (!out.out) fail("missing --out <site-dir>");
	if (!out.workerWasm) fail("missing --worker-wasm <path>");
	return out;
}

function fail(msg) {
	console.error(`[build-pages] ERROR: ${msg}`);
	process.exit(1);
}

function rewriteBareImports(src) {
	for (const [bare, abs] of Object.entries(BARE_IMPORTS)) {
		const escaped = bare.replace(/[/.-]/g, (c) => "\\" + c);
		const re = new RegExp(
			`((?:from|import)\\s*\\(?\\s*['"])${escaped}(['"]\\)?)`,
			"g",
		);
		src = src.replace(re, (_m, a, b) => a + abs + b);
	}
	return src;
}

// Recursively copy a dir, rewriting bare imports in any JS-ish file.
function copyAndRewrite(src, dst) {
	mkdirSync(dst, { recursive: true });
	for (const entry of readdirSync(src)) {
		const s = join(src, entry);
		const d = join(dst, entry);
		if (statSync(s).isDirectory()) {
			copyAndRewrite(s, d);
		} else if (/\.(js|mjs|cjs)$/.test(entry)) {
			writeFileSync(d, rewriteBareImports(readFileSync(s, "utf8")));
		} else {
			cpSync(s, d);
		}
	}
}

async function main() {
	const { out, workerWasm, coi } = parseArgs(process.argv.slice(2));
	mkdirSync(out, { recursive: true });

	// 1. Trunk-built Dioxus app → site root.
	if (!existsSync(join(DIST, "index.html")))
		fail(`dist/index.html missing — run \`trunk build --release\` first`);
	cpSync(DIST, out, { recursive: true });

	// 2. Worker wasm → the absolute root URL the worker-entry fetches.
	if (!existsSync(workerWasm)) fail(`worker wasm missing: ${workerWasm}`);
	cpSync(workerWasm, join(out, "holon_worker.wasm32-wasi.wasm"));

	// 3. Worker web harness (worker-entry.mjs + OPFS stubs) → /web/.
	const webSrc = join(WORKER_ROOT, "web");
	if (!existsSync(webSrc)) fail(`worker web dir missing: ${webSrc}`);
	copyAndRewrite(webSrc, join(out, "web"));

	// 4. The five ESM deps → /node_modules/, with bare imports rewritten.
	const nmSrc = join(WORKER_ROOT, "node_modules");
	for (const dep of NODE_DEPS) {
		const src = join(nmSrc, dep);
		if (!existsSync(src))
			fail(
				`worker node_modules dep missing: ${src} — run \`npm install\` in holon-worker`,
			);
		copyAndRewrite(src, join(out, "node_modules", dep));
	}

	// 5. coi-serviceworker: patch COOP/COEP so SharedArrayBuffer works on Pages.
	const coiSrc = coi || (await downloadCoi(out));
	cpSync(coiSrc, join(out, "coi-serviceworker.js"));
	injectCoiScript(join(out, "index.html"));

	console.log(`[build-pages] assembled site in ${out}`);
}

// Download coi-serviceworker.min.js into a temp location next to the site.
async function downloadCoi(out) {
	const dest = join(dirname(out), ".coi-serviceworker.min.js");
	console.log(`[build-pages] downloading coi-serviceworker → ${dest}`);
	const res = await fetch(COI_URL);
	if (!res.ok) fail(`coi-serviceworker download failed: HTTP ${res.status}`);
	writeFileSync(dest, await res.text());
	return dest;
}

// Inject the coi-serviceworker script as the FIRST element in <head> so it
// registers before the app bootstraps and can trigger the one-time reload.
function injectCoiScript(indexHtml) {
	if (!existsSync(indexHtml)) fail(`index.html missing at ${indexHtml}`);
	let html = readFileSync(indexHtml, "utf8");
	const tag = '<script src="/coi-serviceworker.js"></script>';
	if (html.includes("coi-serviceworker.js")) {
		console.log("[build-pages] coi-serviceworker already injected; skipping");
		return;
	}
	html = html.replace(/<head([^>]*)>/i, (m) => `${m}\n    ${tag}`);
	writeFileSync(indexHtml, html);
}

main().catch((e) => {
	console.error(e);
	process.exit(1);
});
