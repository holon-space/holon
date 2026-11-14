---
id: 2026-11-14-mcp-org-file-reads-escape-the-vault
date: 2026-11-14
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The MCP tools `read_org_file` and `render_org` read any existing file a path
  argument named, inside the vault or not, and `holon-mcp --http` served a
  non-loopback address although the server has no login.
---

## Bug
Found by a code audit of the MCP server's trust boundary, not by a test.
A path-like `doc_id` that named an existing file was read as it was, so an
absolute path or a `../` spelling reached files outside the vault. A symlink
inside the vault reached its target wherever that was. A session without a
vault root read absolute paths as well. `--http` bound any address it was
given.

## Root cause
`resolve_to_file_path` (frontends/mcp/src/tools.rs) returned a path-like
argument unchanged when the file existed, before it tried the vault root, and
returned an alias target without a check. `run_http_server`
(frontends/mcp/src/di.rs) bound its address without a check.

## Missing piece
- Litmus COVERAGE: no generator drew a document argument outside the vault:
  the MCP tool tests used vault files and registered aliases only.
- Litmus ORACLE: no property said that a read returns only bytes of a file
  inside the vault, or that the HTTP server listens only on loopback.

## Remedy
Every read resolves through `VaultPath::inside_canonical`
(crates/holon-filesystem/src/vault_path.rs): the spelling must exist, and its
canonical path, links resolved, must lie strictly inside the canonical vault
root. The read opens the canonical path. A session without a vault root reads
nothing. `run_http_server` and the `--http` flag refuse a non-loopback
address before any socket opens. Property tests draw path spellings (`..`,
`.`, links, a name-prefix sibling, absolute and relative) against both the
real and the in-memory filesystem, and every IP address against the bind
check. A process test shows the binary exits non-zero on `--http 0.0.0.0`
without a listener.
