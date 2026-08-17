#!/usr/bin/env bash
# holon-integration-enable.sh — switch a bundled Holon integration ON.
#
# Enablement lives in `{config_dir}/integrations/<provider>.state.toml`, read by
# `holon_mcp_client::IntegrationConfigStore`. Copying a sidecar YAML into that
# directory enables NOTHING; this file is the switch.
#
# Usage:
#   scripts/holon-integration-enable.sh <provider>
#   scripts/holon-integration-enable.sh <provider> <client-id-file> <client-secret-file> <refresh-token-file>
#
# With no credential paths the integration is recorded as enabled but
# `unconfigured`. With them it also records WHERE the credentials live — never a
# secret value; the state file is plain-text user config.
#
# Env:
#   HOLON_MCP_INTEGRATIONS_DIR   the app's own variable for this directory
#                                (default: $HOME/.config/holon/integrations)

set -euo pipefail

die() {
  echo "error: $*" >&2
  exit 1
}

[ $# -eq 1 ] || [ $# -eq 4 ] || die "usage: $0 <provider> [<client-id-file> <client-secret-file> <refresh-token-file>]"

PROVIDER="$1"
DIR="${HOLON_MCP_INTEGRATIONS_DIR:-$HOME/.config/holon/integrations}"
STATE_FILE="$DIR/${PROVIDER}.state.toml"

# Presence is a compile-time fact: a state file for a provider the build does not
# ship is read by nothing, so writing one would report success and do nothing.
BUNDLED_SRC="$(dirname "$0")/../crates/holon-mcp-client/src/bundled_sidecars.rs"
[ -r "$BUNDLED_SRC" ] || die "cannot read $BUNDLED_SRC — run this from a Holon checkout"
BUNDLED="$(sed -n 's/^ *bundled!("\(.*\)"),$/\1/p' "$BUNDLED_SRC")"
[ -n "$BUNDLED" ] || die "found no bundled providers in $BUNDLED_SRC — its format changed"
if ! printf '%s\n' "$BUNDLED" | grep -qxF "$PROVIDER"; then
  die "this build ships no integration '$PROVIDER'. Bundled: $(printf '%s' "$BUNDLED" | tr '\n' ' ')"
fi

mkdir -p "$DIR"

if [ $# -eq 4 ]; then
  HOLON_CLIENT_ID_FILE="$2" \
  HOLON_CLIENT_SECRET_FILE="$3" \
  HOLON_REFRESH_TOKEN_FILE="$4" \
  HOLON_STATE_FILE="$STATE_FILE" \
    python3 -c '
import json, os

def q(s):
    return json.dumps(s)

path = os.environ["HOLON_STATE_FILE"]
with open(path, "w") as f:
    f.write("schema_version = 1\n")
    f.write("enabled = true\n\n")
    f.write("[configuration]\n")
    f.write("status = \"configured\"\n")
    f.write("refresh_token_file = %s\n\n" % q(os.environ["HOLON_REFRESH_TOKEN_FILE"]))
    f.write("[configuration.client_id]\n")
    f.write("source = \"file\"\n")
    f.write("path = %s\n\n" % q(os.environ["HOLON_CLIENT_ID_FILE"]))
    f.write("[configuration.client_secret]\n")
    f.write("source = \"file\"\n")
    f.write("path = %s\n" % q(os.environ["HOLON_CLIENT_SECRET_FILE"]))
'
else
  HOLON_STATE_FILE="$STATE_FILE" python3 -c '
import os

with open(os.environ["HOLON_STATE_FILE"], "w") as f:
    f.write("schema_version = 1\n")
    f.write("enabled = true\n\n")
    f.write("[configuration]\n")
    f.write("status = \"unconfigured\"\n")
'
fi

echo "Enabled '$PROVIDER' — wrote $STATE_FILE" >&2
echo "Restart Holon to pick it up." >&2
