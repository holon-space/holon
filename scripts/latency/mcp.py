#!/usr/bin/env python3
"""Minimal stdlib streamable-HTTP MCP client for the Holon app."""
import json
import sys
import urllib.request


class Mcp:
    def __init__(self, port):
        self.url = f"http://127.0.0.1:{port}/mcp"
        self.sid = None
        self.n = 0
        r = self._post({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "holon-latency", "version": "1"}}}, want_sid=True)
        assert "result" in r, r
        self._post({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
                   notify=True)

    def _post(self, body, want_sid=False, notify=False):
        data = json.dumps(body).encode()
        req = urllib.request.Request(self.url, data=data, headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            **({"mcp-session-id": self.sid} if self.sid else {})})
        with urllib.request.urlopen(req, timeout=120) as resp:
            if want_sid:
                self.sid = resp.headers.get("mcp-session-id")
            raw = resp.read().decode()
        if notify or not raw.strip():
            return {}
        for line in raw.splitlines():
            if line.startswith("data: "):
                return json.loads(line[6:])
        return json.loads(raw)

    def call(self, name, args):
        self.n += 1
        r = self._post({"jsonrpc": "2.0", "id": self.n, "method": "tools/call",
                        "params": {"name": name, "arguments": args}})
        if "error" in r:
            raise RuntimeError(f"{name} -> {r['error']}")
        content = r["result"].get("content", [])
        return "\n".join(c.get("text", "") for c in content if c.get("type") == "text")

    def tools(self):
        r = self._post({"jsonrpc": "2.0", "id": 999, "method": "tools/list", "params": {}})
        return [t["name"] for t in r["result"]["tools"]]


if __name__ == "__main__":
    m = Mcp(sys.argv[1])
    if sys.argv[2] == "--list":
        print("\n".join(m.tools()))
    else:
        print(m.call(sys.argv[2], json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}))
