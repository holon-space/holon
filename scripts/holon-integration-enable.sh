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

# A state file for a name NOTHING provides is read by nothing, so writing one
# would report success and do nothing.
#
# The question is ASKED OF THE BINARY rather than answered here. This script
# used to parse `bundled_sidecars.rs` and glob `*.yaml`, which is a second
# implementation of the loader's admission rules — and it drifted: the loader
# refuses a SYMLINKED sidecar and the glob switched one on, so the script and
# the app disagreed about which connections exist.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -n "${HOLON_CONNECTION_BIN:-}" ]; then
  ASK=("$HOLON_CONNECTION_BIN")
elif [ -x "$ROOT/target/release/holon-connection" ]; then
  ASK=("$ROOT/target/release/holon-connection")
elif [ -x "$ROOT/target/debug/holon-connection" ]; then
  ASK=("$ROOT/target/debug/holon-connection")
elif [ -r "$ROOT/Cargo.toml" ]; then
  ASK=(cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -p holon-mcp-client \
       --bin holon-connection --)
else
  die "cannot find holon-connection: set HOLON_CONNECTION_BIN to the built binary, or run this \
from a Holon checkout so it can be built. This script does not decide for itself which \
connections exist — the loader does, and that is what keeps the two from disagreeing."
fi

# stderr passes through: it names each file the loader ignores and why, which is
# what tells a user their file is there and still does nothing.
PROVIDERS="$("${ASK[@]}" list "$DIR")" \
  || die "holon-connection could not list the connections in $DIR"

if ! printf '%s\n' "$PROVIDERS" | grep -qxF "$PROVIDER"; then
  die "nothing provides an integration '$PROVIDER'. This build admits: $(printf '%s' \
"$PROVIDERS" | tr '\n' ' '). If you installed '$PROVIDER.yaml' in $DIR and it is not listed, the \
reason is on the lines above."
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
