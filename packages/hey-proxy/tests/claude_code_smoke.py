#!/usr/bin/env python3
"""Offline integration with an installed Claude Code; no subscription/login needed.

uv run --with cryptography tests/claude_code_smoke.py target/debug/hey-proxy
Add --dashboard to keep the synthetic dashboard running for visual inspection.
"""
import contextlib
import http.server
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from cryptography.hazmat.primitives.ciphers.aead import AESGCMSIV
import base64

TOKEN = "sk-ant-oat01-synthetic-proxy-owned"
requests = []


class Upstream(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def respond(self, value):
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        assert self.headers.get("Authorization") == f"Bearer {TOKEN}"
        self.respond({
            "five_hour": {"utilization": 37.5, "resets_at": "2026-10-01T03:00:00Z"},
            "seven_day": {"utilization": 64, "resets_at": "2026-10-04T03:00:00Z"},
            "seven_day_sonnet": {"utilization": 18, "resets_at": "2026-10-04T03:00:00Z"},
            "extra_usage": {"is_enabled": False},
        })

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        assert self.headers.get("Authorization") == f"Bearer {TOKEN}"
        assert self.headers.get("x-api-key") is None
        assert "oauth-2025-04-20" in self.headers.get("anthropic-beta", "")
        requests.append({"path": self.path, "model": body.get("model"), "stream": body.get("stream")})
        if self.path.split("?")[0].endswith("count_tokens"):
            self.respond({"input_tokens": 20})
            return
        message = {"type": "message", "id": "msg_synthetic", "role": "assistant", "model": body["model"], "content": [], "stop_reason": None, "stop_sequence": None, "usage": {"input_tokens": 20, "output_tokens": 0}}
        if not body.get("stream"):
            self.respond({**message, "content": [{"type": "text", "text": "PROXY_OK"}], "stop_reason": "end_turn", "usage": {"input_tokens": 20, "output_tokens": 3}})
            return
        events = [
            {"type": "message_start", "message": message},
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "PROXY_OK"}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 3}},
            {"type": "message_stop"},
        ]
        data = "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main():
    binary = pathlib.Path(sys.argv[1]).resolve()
    dashboard = "--dashboard" in sys.argv
    with tempfile.TemporaryDirectory(prefix="hey-proxy-claude-test-") as tmp:
        root = pathlib.Path(tmp)
        key, nonce = os.urandom(32), os.urandom(12)
        tokens = json.dumps({"access_token": TOKEN, "refresh_token": "synthetic-refresh", "expires_at": int(time.time()) + 3600}).encode()
        encrypted = AESGCMSIV(key).encrypt(nonce, tokens, b"hey-proxy Claude OAuth v1")
        for name, data in [("config.claude.key", key), ("config.claude.json", json.dumps({"encrypted": "v1:" + base64.b64encode(nonce + encrypted).decode()}).encode())]:
            path = root / name
            path.write_bytes(data)
            path.chmod(0o600)
        upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        threading.Thread(target=upstream.serve_forever, daemon=True).start()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        config = root / "config.json"
        config.write_text(json.dumps({"listen": f"127.0.0.1:{port}", "providers": {"claude": {"upstream_url": f"http://127.0.0.1:{upstream.server_port}"}}}))
        proxy = subprocess.Popen([str(binary), "--config", str(config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        url = f"http://127.0.0.1:{port}"
        try:
            for _ in range(400):
                if proxy.poll() is not None:
                    raise RuntimeError("Synthetic proxy failed: " + proxy.stderr.read().decode())
                try:
                    with urllib.request.urlopen(url + "/overview/api", timeout=1):
                        break
                except OSError:
                    time.sleep(0.05)
            else:
                raise RuntimeError("Proxy did not start")
            if dashboard:
                print(f"Synthetic dashboard: {url}", flush=True)
                while True:
                    time.sleep(1)
            claude = shutil.which("claude")
            assert claude, "Install Claude Code to run this optional smoke test"
            # Isolate Claude's settings and disable unrelated traffic. Real credentials
            # are never loaded or used; both client and upstream tokens are synthetic.
            env = {k: v for k, v in os.environ.items() if not k.startswith(("ANTHROPIC_", "CLAUDE_", "CLAUDECODE"))}
            env.update(ANTHROPIC_BASE_URL=url, ANTHROPIC_AUTH_TOKEN="local-placeholder",
                       CLAUDE_CONFIG_DIR=str(root / "client"), CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1")
            result = subprocess.run([claude, "-p", "Reply PROXY_OK.", "--model", "sonnet", "--tools", "", "--no-session-persistence", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}'], cwd=root, env=env, capture_output=True, text=True, timeout=60)
            assert result.returncode == 0 and "PROXY_OK" in result.stdout, f"Claude smoke failed: exit {result.returncode}; {result.stderr[-1000:]}"
            assert any(r["stream"] for r in requests), "Claude did not send a native streaming request"
            with urllib.request.urlopen(url + "/claude/usage") as response:
                usage = json.load(response)
            assert usage["data"]["windows"][0]["used_percent"] == 37.5
            assert TOKEN not in json.dumps(usage)
            print("PASS: installed Claude Code → hey-proxy → synthetic Claude upstream; proxy-owned token, streaming and subscription limits")
        finally:
            proxy.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                proxy.wait(timeout=5)
            if proxy.poll() is None:
                proxy.kill()
                proxy.wait()
            upstream.shutdown()


if __name__ == "__main__":
    main()
