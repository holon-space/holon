---
id: 2026-07-21-share-enrollment-capability-verified-acceptor-typed
date: 2026-07-21
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Share-enrollment capability NOT verified at acceptor — typed capability
  state machine + Ticket v2 landed (CSPRNG secret, constant-time blake3 proof,
  AuthorizedPeer witness; holon-loro 210/0) but accept_shared_subtree
  (loro_share_backend.rs:1449) decodes WITHOUT enforcing; bearer
  shared_tree_id still leaks via ALPN/logs/SQL/peers-sidecar → leaked id +
  self-minted v2 ticket can enroll until the gate is wired (disclosed in
  code).
source_line: 1064
---

## Bug

Share-enrollment capability NOT verified at acceptor — typed capability
state machine + Ticket v2 landed (CSPRNG secret, constant-time blake3 proof,
AuthorizedPeer witness; holon-loro 210/0) but accept_shared_subtree
(loro_share_backend.rs:1449) decodes WITHOUT enforcing; bearer
shared_tree_id still leaks via ALPN/logs/SQL/peers-sidecar → leaked id +
self-minted v2 ticket can enroll until the gate is wired (disclosed in
code).

## Missing piece

wire capability verification into accept (awaits enrollment-ceremony ruling:
transport TOFU-vs-SAS, roster signing/persistence); no live-QUIC enrollment
test (state machine + wire round-trip only)

## Remedy

OPEN (W4; acceptor gate deferred + disclosed, ruling queued)
