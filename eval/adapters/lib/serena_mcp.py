"""Minimal MCP stdio client for Serena: one server per question.

Serena has no natural language search. The closest entry point is find_symbol
with substring matching, so each content word of the question (light suffix
stripping, stopwords dropped) is looked up as a symbol name fragment. Symbols are
ranked by how many distinct words their name contains; ties keep the shorter
name, then Serena's own order. find_symbol lines are 0-based LSP lines and are
printed 1-based.

Usage: serena_mcp.py <serena executable> <corpus root> <question> <limit>
"""

import json
import re
import subprocess
import sys

STOPWORDS = set(
    "a an and are as at be by can do does for from get gets has have how in into is it its of on or that the "
    "their them then there this to was what when where which while who why with without".split()
)


def words(question):
    out = []
    for w in re.findall(r"[a-z0-9]+", question.lower()):
        if len(w) < 3 or w in STOPWORDS:
            continue
        for suffix in ("ing", "ed", "es", "s"):
            if w.endswith(suffix) and len(w) - len(suffix) >= 4:
                w = w[: -len(suffix)]
                break
        if w not in out:
            out.append(w)
    return out


class Client:
    def __init__(self, cmd):
        self.proc = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
        )
        self.next_id = 0

    def send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("serena exited before answering " + method)
            msg = json.loads(line)
            if msg.get("id") == self.next_id:
                if "error" in msg:
                    raise RuntimeError(json.dumps(msg["error"]))
                return msg["result"]

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def find_symbol(client, term):
    result = client.request(
        "tools/call", {"name": "find_symbol", "arguments": {"name_path_pattern": term, "substring_matching": True}}
    )
    text = "".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")
    if result.get("isError"):
        raise RuntimeError(text)
    try:
        symbols = json.loads(text)
    except ValueError:
        return []  # a plain text notice, e.g. no match
    return symbols if isinstance(symbols, list) else []


def main():
    serena, root, question, limit = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
    terms = words(question)
    if not terms:
        return
    client = Client([
        serena, "start-mcp-server", "--project", root,
        "--enable-web-dashboard", "false", "--open-web-dashboard", "false", "--enable-gui-log-window", "false",
        "--agent-interface", "tools",
    ])
    try:
        client.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "knot-eval", "version": "1"},
        })
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        found = {}
        for term in terms:
            for sym in find_symbol(client, term):
                loc = sym.get("body_location") or sym.get("location") or {}
                if "start_line" not in loc:
                    continue
                key = (sym["relative_path"], loc["start_line"], loc.get("end_line", loc["start_line"]))
                if key not in found:
                    found[key] = (len(found), sym["name_path"].rsplit("/", 1)[-1].lower())
    finally:
        client.close()

    def rank(item):
        order, name = item[1]
        return (-sum(t in name for t in terms), len(name), order)

    for (path, start, end), _ in sorted(found.items(), key=rank)[:limit]:
        print(f"{path}:L{start + 1}-L{end + 1}")


if __name__ == "__main__":
    main()
