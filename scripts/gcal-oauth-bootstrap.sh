#!/usr/bin/env bash
# gcal-oauth-bootstrap.sh — one-time Google Calendar consent flow for Holon.
#
# Runs the OAuth2 "installed app" loopback flow: opens Google's consent page,
# captures the redirect on 127.0.0.1, exchanges the authorization code for a
# long-lived REFRESH token, and writes ONLY that refresh token to
# ~/.config/holon/gcal-refresh-token with mode 0600.
#
# SECURITY:
#   * The refresh token is NEVER printed to the terminal, a log, or argv — it is
#     piped straight into the file writer.
#   * The file is created here (never by Holon) with umask 077 + explicit
#     chmod 600.
#   * The client secret is read from the environment, never echoed.
#
# PREREQUISITES (see docs/integrations/gcal.yaml header for the full walkthrough):
#   * A Google Cloud OAuth client of type "Desktop app" (Calendar API enabled).
#   * Environment:
#       export GCAL_CLIENT_ID='xxxx.apps.googleusercontent.com'
#       export GCAL_CLIENT_SECRET='GOCSPX-...'
#   * python3 and curl on PATH.
#
# Usage:
#   GCAL_CLIENT_ID=... GCAL_CLIENT_SECRET=... scripts/gcal-oauth-bootstrap.sh

set -euo pipefail
umask 077

SCOPE="https://www.googleapis.com/auth/calendar.readonly"
TOKEN_FILE="${HOLON_GCAL_REFRESH_TOKEN_FILE:-$HOME/.config/holon/gcal-refresh-token}"
PORT="${GCAL_OAUTH_PORT:-8765}"
REDIRECT_URI="http://127.0.0.1:${PORT}"
AUTH_ENDPOINT="https://accounts.google.com/o/oauth2/v2/auth"
TOKEN_ENDPOINT="https://oauth2.googleapis.com/token"

die() {
  echo "error: $*" >&2
  exit 1
}

: "${GCAL_CLIENT_ID:?set GCAL_CLIENT_ID (Desktop OAuth client id)}"
: "${GCAL_CLIENT_SECRET:?set GCAL_CLIENT_SECRET (Desktop OAuth client secret)}"
command -v python3 >/dev/null 2>&1 || die "python3 is required"
command -v curl >/dev/null 2>&1 || die "curl is required"

urlencode() {
  python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

STATE="$(python3 -c 'import secrets; print(secrets.token_urlsafe(24))')"
AUTH_URL="${AUTH_ENDPOINT}?response_type=code&client_id=$(urlencode "$GCAL_CLIENT_ID")&redirect_uri=$(urlencode "$REDIRECT_URI")&scope=$(urlencode "$SCOPE")&access_type=offline&prompt=consent&state=$(urlencode "$STATE")"

echo "Opening the Google consent page in your browser..." >&2
echo "If it does not open, paste this URL manually:" >&2
echo "$AUTH_URL" >&2
if command -v open >/dev/null 2>&1; then
  open "$AUTH_URL" || true
elif command -v xdg-open >/dev/null 2>&1; then
  xdg-open "$AUTH_URL" || true
fi

echo "Waiting for the redirect on ${REDIRECT_URI} ..." >&2

# One-shot loopback listener: accept a single request, verify the CSRF state,
# return a friendly page, and print ONLY the authorization code to stdout.
CODE="$(
  EXPECTED_STATE="$STATE" python3 - "$PORT" <<'PY'
import http.server, os, sys, urllib.parse

port = int(sys.argv[1])
expected_state = os.environ["EXPECTED_STATE"]
captured = {"code": None, "error": None}

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):  # silence default stderr logging
        pass

    def do_GET(self):
        q = urllib.parse.urlparse(self.path).query
        params = urllib.parse.parse_qs(q)
        state = params.get("state", [None])[0]
        if state != expected_state:
            captured["error"] = "state mismatch (possible CSRF) — aborting"
        elif "error" in params:
            captured["error"] = params["error"][0]
        else:
            captured["code"] = params.get("code", [None])[0]
        body = b"Holon: authorization received. You can close this tab and return to the terminal."
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

httpd = http.server.HTTPServer(("127.0.0.1", port), Handler)
httpd.handle_request()
if captured["error"]:
    print("", end="")
    sys.stderr.write("error: %s\n" % captured["error"])
    sys.exit(3)
if not captured["code"]:
    sys.stderr.write("error: no authorization code in redirect\n")
    sys.exit(3)
sys.stdout.write(captured["code"])
PY
)"

[ -n "$CODE" ] || die "did not capture an authorization code"

echo "Exchanging the authorization code for a refresh token..." >&2

# Exchange the code. The response JSON (which contains the tokens) is captured
# into a variable and piped straight into the writer — never printed.
TOKEN_RESPONSE="$(
  curl -sS -X POST "$TOKEN_ENDPOINT" \
    --data-urlencode "code=${CODE}" \
    --data-urlencode "client_id=${GCAL_CLIENT_ID}" \
    --data-urlencode "client_secret=${GCAL_CLIENT_SECRET}" \
    --data-urlencode "redirect_uri=${REDIRECT_URI}" \
    --data-urlencode "grant_type=authorization_code"
)"

mkdir -p "$(dirname "$TOKEN_FILE")"

# Extract refresh_token and write it 0600, without ever echoing it. The
# extractor is passed via `-c` so the JSON stays on stdin (a heredoc here would
# override the pipe — see shellcheck SC2259).
EXTRACT_PY="$(cat <<'PY'
import json, os, sys

try:
    data = json.load(sys.stdin)
except Exception:
    sys.stderr.write("error: token endpoint returned a non-JSON response\n")
    sys.exit(4)

if "refresh_token" not in data:
    # Do NOT print the response — it may contain an access token.
    err = data.get("error", "no refresh_token in response")
    desc = data.get("error_description", "")
    sys.stderr.write(
        "error: %s %s\n"
        "(Google only returns a refresh_token with access_type=offline and a fresh consent. "
        "Revoke prior access at https://myaccount.google.com/permissions and retry.)\n"
        % (err, desc)
    )
    sys.exit(4)

path = os.environ["HOLON_TOKEN_FILE"]
# Create private from the start (umask 077 is already set by the shell).
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f:
    f.write(data["refresh_token"].strip() + "\n")
os.chmod(path, 0o600)
PY
)"
printf '%s' "$TOKEN_RESPONSE" | HOLON_TOKEN_FILE="$TOKEN_FILE" python3 -c "$EXTRACT_PY"

# Defense in depth: ensure perms regardless of prior state.
chmod 600 "$TOKEN_FILE"

# Scrub the secrets from this shell's memory.
unset TOKEN_RESPONSE CODE GCAL_CLIENT_SECRET

echo "Success. Refresh token written to: $TOKEN_FILE" >&2
echo "Permissions:" >&2
ls -l "$TOKEN_FILE" >&2
echo "Now: cp docs/integrations/gcal.yaml ~/.config/holon/integrations/ and restart Holon." >&2
