#!/usr/bin/env python3
"""Minimal streamable-HTTP MCP client for driving a running Holon frontend.

Stdlib + `requests` only. Full initialize handshake per invocation, then ONE
tools/call, result printed to stdout.

Usage:
    python3 holon_mcp_cli.py <port> <tool_name> ['<json_args>'] [--out file.png]
    python3 holon_mcp_cli.py <port> --list
    python3 holon_mcp_cli.py <port> --health

SAFETY: talks ONLY to 127.0.0.1:<port>. Never use 8520 (Martin's live app).
"""
import base64, json, sys, requests
PROTO = "2025-03-26"

def _post(url, sid, payload):
    h = {"Content-Type": "application/json",
         "Accept": "application/json, text/event-stream"}
    if sid: h["Mcp-Session-Id"] = sid
    r = requests.post(url, headers=h, data=json.dumps(payload), timeout=60)
    r.raise_for_status()
    # Defense against charset-less servers: `requests` falls back to Latin-1
    # for a bare `text/event-stream`/`application/json` Content-Type (no
    # `charset` param), mojibaking every multibyte character in the body.
    # The MCP server response is always UTF-8; force it explicitly rather
    # than trusting the (possibly charset-less) header.
    r.encoding = "utf-8"
    sid = r.headers.get("Mcp-Session-Id", sid)
    body, result = r.text, None
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
        r = requests.get(f"http://127.0.0.1:{port}/health", timeout=5)
        print(f"HTTP {r.status_code}: {r.text}"); return
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
