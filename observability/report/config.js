// Data resolution for the report.
//
// - `?data=<url-or-relative-path>` always overrides.
// - Local (localhost / 127.0.0.1 / file:): the report reads `localDataPath` and
//   `capturesIndexPath` — absolute paths from the served root. `just obs-serve`
//   serves the REPO root, so the gitignored `.observability/` data dir is
//   reachable at the paths below.
// - Remote (GitHub Pages): the report reads `runs.json` RELATIVE to this page —
//   the deploy workflow (`.github/workflows/pages.yml`) downloads the release
//   asset and co-locates it with the report, so it is served same-origin and no
//   CORS applies. (GitHub release assets are NOT browser-fetchable cross-origin:
//   the blob host sends no Access-Control-Allow-Origin header — verified.)
window.HOLON_OBS_CONFIG = {
	// Local data path, absolute from the served root.
	localDataPath: "/.observability/runs.json",
	// Local captures manifest (flipbook index) — absolute from the served root.
	capturesIndexPath: "/.observability/captures.json",
};
