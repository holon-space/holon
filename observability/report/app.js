/* Holon observability report — static, offline, dependency-free.
 *
 * Data model: `runs.json` is the cumulative index, one entry per run, each the
 * FULL canonical record (see scripts/holon_obs.py). The report fetches exactly
 * one file for run data, plus a second (`captures.json`) for the flipbook of
 * GPUI pixel frames — no server, no build step.
 *
 * Resolution order for the data URL:
 *   1. `?data=<url-or-relative-path>` — explicit override.
 *   2. local: `runs.json` via `localDataPath` (served by `just obs-serve`).
 *   3. remote (Pages): `runs.json` relative to this page — the deploy workflow
 *      co-locates it, so it is served same-origin (release assets are NOT
 *      browser-fetchable: no CORS header on the blob host).
 *
 * Visual evidence (captures) is local-only: the frame manifest is fetched only
 * when served from localhost/file.
 */
(() => {
	const CFG = window.HOLON_OBS_CONFIG || {};
	const SVG_NS = "http://www.w3.org/2000/svg";

	const el = (id) => document.getElementById(id);

	function h(tag, cls, text) {
		const n = document.createElement(tag);
		if (cls) n.className = cls;
		if (text !== null && text !== undefined) n.textContent = text;
		return n;
	}

	// ---- data URL resolution ------------------------------------------------
	function localIndexPath() {
		if (CFG.localDataPath) {
			return new URL(CFG.localDataPath, document.baseURI || location.href).href;
		}
		const here = new URL(".", document.baseURI || location.href);
		return new URL("runs.json", here).href;
	}

	function isLocalHost() {
		return (
			location.protocol === "file:" ||
			location.hostname === "localhost" ||
			location.hostname === "127.0.0.1" ||
			location.hostname === "[::1]"
		);
	}

	function resolveDataUrl() {
		const q = new URLSearchParams(location.search).get("data");
		if (q) return q;
		if (isLocalHost()) return localIndexPath();
		// Remote (GitHub Pages): the deploy workflow co-locates runs.json with
		// this page, so fetch it same-origin (relative). Release assets are NOT
		// browser-fetchable cross-origin (no CORS header on the blob host).
		const here = new URL(".", document.baseURI || location.href);
		return new URL("runs.json", here).href;
	}

	// ---- tiny SVG sparkline -------------------------------------------------
	function sparkline(values, colorFn, opts) {
		const o = opts || {};
		const w = o.w || 360;
		const hgt = o.h || 80;
		const pad = 6;
		const min = Math.min(...values, 0);
		const max = Math.max(...values, 1);
		const span = max - min || 1;
		const n = values.length;

		const svg = document.createElementNS(SVG_NS, "svg");
		svg.setAttribute("viewBox", `0 0 ${w} ${hgt}`);
		svg.setAttribute("width", "100%");
		svg.setAttribute("height", String(hgt));
		svg.setAttribute("preserveAspectRatio", "none");

		const x = (i) =>
			pad + (n <= 1 ? (w - 2 * pad) / 2 : (i / (n - 1)) * (w - 2 * pad));
		const y = (v) => hgt - pad - ((v - min) / span) * (hgt - 2 * pad);

		if (n > 1) {
			const path = [];
			for (let i = 0; i < n; i++) {
				const cmd = i === 0 ? "M" : "L";
				path.push(`${cmd}${x(i).toFixed(1)} ${y(values[i]).toFixed(1)}`);
			}
			const line = document.createElementNS(SVG_NS, "path");
			line.setAttribute("d", path.join(" "));
			line.setAttribute("fill", "none");
			line.setAttribute("stroke", "var(--accent)");
			line.setAttribute("stroke-width", "1.5");
			svg.appendChild(line);
		}

		for (let j = 0; j < n; j++) {
			const c = document.createElementNS(SVG_NS, "circle");
			c.setAttribute("cx", x(j).toFixed(1));
			c.setAttribute("cy", y(values[j]).toFixed(1));
			c.setAttribute("r", n > 30 ? "1.5" : "3");
			c.setAttribute("fill", colorFn ? colorFn(j) : "var(--accent)");
			const t = document.createElementNS(SVG_NS, "title");
			t.textContent = o.labels ? o.labels[j] : String(values[j]);
			c.appendChild(t);
			svg.appendChild(c);
		}
		return svg;
	}

	// ---- rendering ----------------------------------------------------------
	function fmtDur(ms) {
		if (ms === null || ms === undefined) return "—";
		if (ms < 1000) return `${ms} ms`;
		if (ms < 60000) return `${(ms / 1000).toFixed(1)} s`;
		return `${(ms / 60000).toFixed(1)} min`;
	}

	function fmtTime(iso) {
		if (!iso) return "—";
		const d = new Date(iso);
		return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
	}

	function shortGit(rev) {
		return rev ? rev.slice(0, 7) : "—";
	}

	function verdictClass(v) {
		if (v === "green") return "green";
		if (v === "red") return "red";
		return "muted";
	}

	function metric(id, label, value, cls) {
		const c = el(id);
		c.textContent = "";
		c.appendChild(h("div", "metric-label", label));
		c.appendChild(h("div", `metric-value ${cls || ""}`, value));
	}

	function renderSummary(runs) {
		el("summary").hidden = false;
		const total = runs.length;
		const green = runs.filter((r) => r.verdict === "green").length;
		const red = runs.filter((r) => r.verdict === "red").length;
		const last = runs[runs.length - 1];

		metric("card-runs", "runs", String(total));
		metric("card-green", "green", String(green), "good");
		metric("card-red", "red", String(red), red ? "bad" : "good");
		metric(
			"card-verdict",
			"latest",
			last ? last.verdict : "—",
			last ? verdictClass(last.verdict) : "",
		);

		const lastLine = last
			? `${last.kind} · ${fmtTime(last.started_at)}`
			: "no runs yet";
		metric("card-lastrun", "last run", lastLine);

		// Direction of travel: red count in the latest half vs the older half.
		let trend;
		let trendCls;
		if (runs.length >= 2) {
			const mid = Math.floor(runs.length / 2);
			const olderRed = runs
				.slice(0, mid)
				.filter((r) => r.verdict === "red").length;
			const newerRed = runs
				.slice(mid)
				.filter((r) => r.verdict === "red").length;
			if (newerRed < olderRed) {
				trend = "improving";
				trendCls = "good";
			} else if (newerRed > olderRed) {
				trend = "worsening";
				trendCls = "bad";
			} else {
				trend = "steady";
				trendCls = "";
			}
			trend = `${trend}  (${olderRed}→${newerRed} red)`;
		} else {
			trend = "need ≥2 runs";
			trendCls = "";
		}
		metric("card-trend", "direction", trend, trendCls);
	}

	function renderCharts(runs) {
		el("history-section").hidden = false;
		const dur = runs.map((r) => r.duration_ms || 0);
		const pan = runs.map((r) => r.panic_count || 0);
		const labels = runs.map(
			(r) => `${fmtTime(r.started_at)} · ${r.kind} · ${r.verdict}`,
		);
		const colorFn = (i) =>
			runs[i].verdict === "green" ? "var(--green)" : "var(--red)";

		const durBox = el("chart-duration");
		durBox.textContent = "";
		durBox.appendChild(sparkline(dur, colorFn, { labels }));

		const panBox = el("chart-panics");
		panBox.textContent = "";
		panBox.appendChild(sparkline(pan, colorFn, { labels }));
	}

	function activeKinds() {
		return Array.from(document.querySelectorAll("#filters .chip.active")).map(
			(b) => b.dataset.kind,
		);
	}

	function renderFilters(runs) {
		const kinds = {};
		runs.forEach((r) => {
			kinds[r.kind] = (kinds[r.kind] || 0) + 1;
		});
		const wrap = el("filters");
		wrap.textContent = "";
		Object.keys(kinds)
			.sort()
			.forEach((k) => {
				const b = h("button", "chip active", `${k} (${kinds[k]})`);
				b.dataset.kind = k;
				b.addEventListener("click", () => {
					b.classList.toggle("active");
					renderRuns(runs, activeKinds());
				});
				wrap.appendChild(b);
			});
	}

	function renderRuns(runs, kinds) {
		const body = el("runs-body");
		body.textContent = "";
		el("runs-section").hidden = false;
		const rows = runs.slice().reverse(); // newest first
		rows.forEach((r) => {
			if (kinds.length && kinds.indexOf(r.kind) === -1) return;
			const tr = h("tr");
			tr.appendChild(h("td", "", fmtTime(r.started_at)));
			tr.appendChild(h("td", "", r.kind));
			tr.appendChild(h("td", `verdict ${verdictClass(r.verdict)}`, r.verdict));
			tr.appendChild(
				h(
					"td",
					"num",
					String(r.cases === null || r.cases === undefined ? "" : r.cases),
				),
			);
			tr.appendChild(h("td", "num", fmtDur(r.duration_ms)));
			tr.appendChild(h("td", "num", String(r.panic_count || 0)));
			tr.appendChild(h("td", "", r.host || "—"));
			tr.appendChild(h("td", "mono", shortGit(r.git_rev)));
			tr.addEventListener("click", () => renderDetail(r));
			body.appendChild(tr);
		});
	}

	function renderDetail(r) {
		el("detail-section").hidden = false;
		const d = el("detail");
		d.textContent = "";

		const grid = h("div", "detail-grid");
		const fields = [
			["run_id", r.run_id],
			["kind", r.kind],
			["verdict", r.verdict],
			["started", fmtTime(r.started_at)],
			["duration", fmtDur(r.duration_ms)],
			["cases", r.cases === null || r.cases === undefined ? "—" : r.cases],
			["host", r.host || "—"],
			["git", r.git_rev || "—"],
		];
		fields.forEach((kv) => {
			const row = h("div", "detail-row");
			row.appendChild(h("span", "detail-k", kv[0]));
			row.appendChild(
				h("span", "detail-v mono", kv[1] === null ? "" : String(kv[1])),
			);
			grid.appendChild(row);
		});
		d.appendChild(grid);

		const panics = r.panics || [];
		d.appendChild(h("h3", "", `Panics (${panics.length})`));
		if (!panics.length) {
			d.appendChild(h("p", "muted", "none"));
		} else {
			panics.forEach((p) => {
				const pre = h("pre", "panic");
				pre.textContent = `${p.location}\n${p.message}`;
				d.appendChild(pre);
			});
		}
	}

	function showError(msg) {
		const s = el("source-badge");
		s.textContent = msg;
		s.classList.add("error");
		document
			.querySelector("main")
			.appendChild(
				h(
					"p",
					"error",
					`Could not load ${msg}. Serve locally with \`just obs-serve\` or pass \`?data=<url>\`.`,
				),
			);
	}

	// ---- visual evidence (flipbook) -----------------------------------------
	const captureState = {
		captures: [],
		current: null,
		idx: 0,
		timer: null,
		baseUrl: null,
	};

	function frameUrl(cap, ev) {
		// captures.json lives at <base>/captures.json; frames live at
		// <base>/captures/<id>/<png>.
		return new URL(`${cap.dir}/${ev.png}`, captureState.baseUrl).href;
	}

	function renderCaptures(captures) {
		const section = el("captures-section");
		if (!captures || !captures.length) {
			section.hidden = true;
			return;
		}
		section.hidden = false;
		captureState.captures = captures;
		const sel = el("capture-select");
		sel.textContent = "";
		captures.forEach((c) => {
			const o = h("option", "", `${c.id} (${c.frames} frames)`);
			o.value = c.id;
			sel.appendChild(o);
		});
		sel.addEventListener("change", () => {
			const cap = captures.find((c) => c.id === sel.value);
			if (cap) loadCapture(cap);
		});
		loadCapture(captures[0]);
	}

	function loadCapture(cap) {
		captureState.current = cap;
		captureState.idx = 0;
		stopPlayback();
		const slider = el("capture-slider");
		slider.max = String(Math.max(0, (cap.events || []).length - 1));
		slider.value = "0";
		showFrame(0);
	}

	function showFrame(i) {
		const cap = captureState.current;
		const events = cap ? cap.events || [] : [];
		const img = el("capture-img");
		if (!events.length) {
			img.hidden = true;
			el("capture-empty").hidden = false;
			el("capture-label").textContent = "";
			return;
		}
		const j = Math.max(0, Math.min(i, events.length - 1));
		captureState.idx = j;
		const ev = events[j];
		img.src = frameUrl(cap, ev);
		img.hidden = false;
		el("capture-empty").hidden = true;
		el("capture-slider").value = String(j);
		el("capture-label").textContent = `${j + 1}/${events.length} · ${ev.label}`;
	}

	function stopPlayback() {
		if (captureState.timer) {
			clearInterval(captureState.timer);
			captureState.timer = null;
		}
		el("capture-play").textContent = "▶ Play";
	}

	function togglePlayback() {
		const cap = captureState.current;
		const n = cap ? (cap.events || []).length : 0;
		if (n < 2) return;
		if (captureState.timer) {
			stopPlayback();
			return;
		}
		el("capture-play").textContent = "⏸ Pause";
		captureState.timer = setInterval(() => {
			showFrame((captureState.idx + 1) % n);
		}, 500);
	}

	function wireCaptureControls() {
		el("capture-prev").addEventListener("click", () => {
			stopPlayback();
			showFrame(captureState.idx - 1);
		});
		el("capture-next").addEventListener("click", () => {
			stopPlayback();
			showFrame(captureState.idx + 1);
		});
		el("capture-play").addEventListener("click", togglePlayback);
		el("capture-slider").addEventListener("input", (e) => {
			stopPlayback();
			showFrame(Number(e.target.value));
		});
	}

	function loadCaptures() {
		if (!CFG.capturesIndexPath || !isLocalHost()) return;
		const capsUrl = new URL(
			CFG.capturesIndexPath,
			document.baseURI || location.href,
		).href;
		captureState.baseUrl = new URL(".", capsUrl).href;
		fetch(capsUrl)
			.then((res) => (res.ok ? res.json() : null))
			.then((idx) => {
				if (idx && idx.captures && idx.captures.length) {
					renderCaptures(idx.captures);
				}
			})
			.catch(() => {});
	}

	// ---- bootstrap ----------------------------------------------------------
	const url = resolveDataUrl();
	const badge = el("source-badge");
	badge.textContent = url;

	fetch(url)
		.then((res) => {
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			return res.json();
		})
		.then((idx) => {
			const runs = (idx.runs || []).slice();
			runs.sort(
				(a, b) =>
					(a.started_at || "").localeCompare(b.started_at || "") ||
					(a.run_id || "").localeCompare(b.run_id || ""),
			);
			el("footer-updated").textContent =
				`index updated ${fmtTime(idx.updated_at)} · ${runs.length} run(s)`;
			renderSummary(runs);
			renderCharts(runs);
			renderFilters(runs);
			renderRuns(runs, []);
		})
		.catch((e) => {
			showError(e && e.message ? e.message : String(e));
		});

	wireCaptureControls();
	loadCaptures();
})();
