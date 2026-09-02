#!/usr/bin/env python3
"""Minimal streamable-HTTP MCP client for driving a running Holon frontend.

Stdlib ONLY — the helper a skill mandates must not need a pip install. Full
initialize handshake per invocation, then ONE tools/call, result printed to
stdout.

Usage:
    python3 holon_mcp_cli.py <port> <tool_name> ['<json_args>'] [--out file.png]
    python3 holon_mcp_cli.py <port> --list
    python3 holon_mcp_cli.py <port> --health

SAFETY: talks ONLY to 127.0.0.1:<port>. Never use 8520 (Martin's live app).
"""
import base64, json, sys, urllib.error, urllib.request
PROTO = "2025-03-26"


def _http(url, method, headers, body=None, timeout=60):
    """One request. Returns (status, headers, text), the body decoded as UTF-8.

    `headers` comes back CASE-INSENSITIVE. HTTP header names are, and the MCP
    session id arrives spelled differently than it is asked for; a plain dict
    loses the session and the next call is refused as an unexpected message.

    The decode is explicit, not sniffed from the Content-Type: the server
    answers UTF-8 whether or not it says so, and the sniffing fallback is
    Latin-1, which mojibakes every multibyte character in a response.
    """
    req = urllib.request.Request(url, method=method, data=body, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            headers = {k.lower(): v for k, v in r.headers.items()}
            return r.status, headers, r.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        raise RuntimeError(
            "HTTP %s from %s: %s" % (e.code, url, e.read().decode("utf-8", "replace"))
        ) from e

def _post(url, sid, payload):
    h = {"Content-Type": "application/json",
         "Accept": "application/json, text/event-stream"}
    if sid: h["Mcp-Session-Id"] = sid
    _, resp_headers, text = _http(url, "POST", h, json.dumps(payload).encode("utf-8"))
    sid = resp_headers.get("mcp-session-id", sid)
    body, result = text, None
    if body.strip().startswith("{"):
        result = json.loads(body)
    else:
        for line in body.splitlines():
            line = line.strip()
            if line.startswith("data:"):
                c = line[5:].strip()
                if c and c != "[DONE]": result = json.loads(c)
    return sid, result

def connect(port):
    url = f"http://127.0.0.1:{port}/mcp"
    sid, _ = _post(url, None, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":PROTO,"capabilities":{},
                  "clientInfo":{"name":"dogfood-explorer","version":"0.1"}}})
    _post(url, sid, {"jsonrpc":"2.0","method":"notifications/initialized"})
    return url, sid

def call(port, tool, args):
    url, sid = connect(port)
    _, resp = _post(url, sid, {"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":tool,"arguments":args}})
    return resp

def list_tools(port):
    url, sid = connect(port)
    _, resp = _post(url, sid, {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    return resp

def main():
    if len(sys.argv) < 3:
        print(__doc__); sys.exit(2)
    port = sys.argv[1]
    if sys.argv[2] == "--health":
        status, _, text = _http(f"http://127.0.0.1:{port}/health", "GET", {}, timeout=5)
        print(f"HTTP {status}: {text}"); return
    if sys.argv[2] == "--list":
        resp = list_tools(port)
        print("\n".join(sorted(t["name"] for t in resp.get("result",{}).get("tools",[])))); return
    tool = sys.argv[2]
    args = json.loads(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] != "--out" else {}
    out = sys.argv[sys.argv.index("--out")+1] if "--out" in sys.argv else None
    resp = call(port, tool, args)
    if resp is None:
        print("ERROR: empty response", file=sys.stderr); sys.exit(1)
    if "error" in resp:
        print(json.dumps(resp["error"], indent=2), file=sys.stderr); sys.exit(1)
    content = resp.get("result",{}).get("content",[])
    if out:
        for c in content:
            if c.get("type") == "image":
                open(out,"wb").write(base64.b64decode(c["data"]))
                print(f"saved image -> {out} ({c.get('mimeType','?')})"); return
        print("no image content", file=sys.stderr); sys.exit(1)
    texts = [c["text"] for c in content if c.get("type")=="text"]
    print("\n".join(texts) if texts else json.dumps(resp.get("result",resp), indent=2))

if __name__ == "__main__":
    main()
