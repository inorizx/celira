#!/usr/bin/env python3
"""本地垫片：给不额外发会话头的客户端补一个头，其余原样转发。

为什么存在：Agent 适配层只带 `Authorization`；有些 OpenAI 兼容端点还要求一个
会话头（缺了会直接 4xx）。这个垫片只做转发，不改请求体、不落盘、不记凭据。

环境变量：
  GATEWAY_UPSTREAM       上游 chat completions 完整地址（必填）
  GATEWAY_SESSION_HEADER 要补的会话头名（缺省 x-opencode-session）
  GATEWAY_PORT           本地监听端口（缺省 18081）
"""
import http.server
import os
import sys
import urllib.error
import urllib.request
import uuid

UPSTREAM = os.environ.get("GATEWAY_UPSTREAM", "")
HEADER = os.environ.get("GATEWAY_SESSION_HEADER", "x-opencode-session")
PORT = int(os.environ.get("GATEWAY_PORT", "18081"))


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length)
        request = urllib.request.Request(UPSTREAM, data=body, method="POST")
        request.add_header("Content-Type", "application/json")
        auth = self.headers.get("authorization")
        if auth:
            request.add_header("Authorization", auth)
        request.add_header(HEADER, f"soak-{uuid.uuid4()}")
        # 有些前置网关按 UA 拦 Python 的默认头。
        request.add_header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) celira-soak/1")
        try:
            with urllib.request.urlopen(request, timeout=300) as upstream:
                data = upstream.read()
                status = upstream.status
        except urllib.error.HTTPError as exc:
            data = exc.read()
            status = exc.code
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    if not UPSTREAM:
        sys.exit("需要 GATEWAY_UPSTREAM：上游 chat completions 的完整地址")
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
