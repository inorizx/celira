#!/usr/bin/env python3
"""Celira 验收桩：一个**脚本化的** OpenAI 兼容 chat completions 端点（只监听 127.0.0.1）。

## 为什么需要它

占位模型钥匙下真模型跑不了，而「五堵墙」要**逐条真跑**（凭据哨兵、超时＋孙进程真死、
40000 字节输出、非法 UTF-8 字节、自杀信号、非零退出码）——这些只有**脚本桩**能确定性驱动。

桩的作用**只是替主体说出我们指定的那句话**（`terminal_run` ＋ 指定命令，外加收场那次
`finish_run`）：跑的是**真 Core、真 App、真进程**，账本（`<家>/env/trace.ndjson`）是主体自己
写的那一份。它不是「真实模型下的端到端」——**语言侧**（模型会不会按事实写 bash 语法）不在它
的范围内，别拿它当那件事的证据。

## 用法

    python3 tools/model_stub.py --port 8123 --script <script.json> [--log <requests.ndjson>]

脚本形状（JSON 数组，**按请求顺序**逐条消费；用尽 ⇒ 一律回 `finish_run`，让活动干净收场）：

    [
      {"tool": "terminal_run", "arguments": {"command": "echo hi"}},
      {"tool": "terminal_run", "arguments": {"command": "sleep 30", "timeout": "1"}}
    ]

- 只使用 Python 3 标准库；**零依赖**（与 `tools/` 里另外两件同一条纪律）。
- 请求体按行落进 `--log`（ndjson）：每一行记第几次请求、字节数、最后一条消息的角色与
  正文长度。**请求头一律不落**（凭据只在头里，取值不进任何输出）。
- 下一刀（Linux 上的 sandbox）要驱动同一类真机验收 ⇒ 这件工具留着复用；它**不进产品**，
  也不被任何测试套件覆盖（它是验收工具，不是被测件）。

## 退出

`SIGTERM` / `SIGINT` / 标准输入读到 EOF 都干净收场（家里那几个进程收场时要它让出端口）。
"""

import argparse
import json
import os
import signal
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# 结束类工具（`finish_run` / `hold_run`）**不带参数**（2026-10-10 裁决；见
# `provider_adapter::finish_schema`）⇒ 脚本用尽时默认那一条也必须空参，否则每一次
# "脚本用完" 都会落成 `system:model-failed / contract-violation`，把干净的收场伪造成失败。
FINISH = {"tool": "finish_run", "arguments": {}}


class Script:
    """按顺序发牌的那副牌（线程安全：请求可能并发，但顺序就是顺序）。"""

    def __init__(self, entries):
        self._entries = list(entries)
        self._lock = threading.Lock()
        self._issued = 0

    def draw(self):
        with self._lock:
            if self._issued < len(self._entries):
                entry = self._entries[self._issued]
            else:
                entry = FINISH
            self._issued += 1
            return self._issued, entry


def response_for(index, entry):
    """把一条脚本条目渲染成 chat completions 的响应体（非流式）。"""
    arguments = json.dumps(entry.get("arguments", {}), ensure_ascii=False)
    body = {
        "id": f"stub-{index}",
        "object": "chat.completion",
        "created": 1_800_000_000,
        "model": "celira-acceptance-stub",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": f"call-{index}",
                            "type": "function",
                            "function": {"name": entry["tool"], "arguments": arguments},
                        }
                    ],
                },
                "finish_reason": "tool_calls",
            }
        ],
        # 三个字段都报（与真厂商同形状）：上下文占用那一格因此不是 unknown。
        "usage": {"prompt_tokens": 1000 + index, "completion_tokens": 20, "total_tokens": 1020 + index},
    }
    return json.dumps(body, ensure_ascii=False).encode("utf-8")


class StubServer(ThreadingHTTPServer):
    """带牌堆与落盘路径的服务器（属性在类型上显式声明，别用动态挂载）。"""

    script: Script
    log_path: str | None

    def __init__(self, address, handler, script: Script, log_path: str | None):
        super().__init__(address, handler)
        self.script = script
        self.log_path = log_path


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "celira-model-stub/1"

    def log_message(self, format, *args):  # 默认那行会把每次请求打到 stderr；这里自己说话
        return

    def _stub(self) -> StubServer:
        server = self.server
        assert isinstance(server, StubServer)
        return server

    def _reply(self, status, payload, content_type="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path == "/health":
            self._reply(200, b'{"ok":true}')
        else:
            self._reply(404, b'{"error":"not found"}')

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if self.path.rstrip("/") != "/chat/completions":
            self._reply(404, b'{"error":"not found"}')
            return
        try:
            request = json.loads(raw.decode("utf-8"))
        except Exception:
            request = {}
        index, entry = self._stub().script.draw()
        self._note(index, raw, request)
        payload = response_for(index, entry)
        sys.stdout.write(f"[stub] 第 {index} 次请求 ⇒ {entry['tool']}\n")
        sys.stdout.flush()
        self._reply(200, payload)

    def _note(self, index, raw, request):
        """把这一次请求的**形状**记一行（正文不进：它是主体的视野，我们只要读数）。"""
        messages = request.get("messages") or []
        last = messages[-1] if messages else {}
        line = {
            "index": index,
            "bytes": len(raw),
            "messages": len(messages),
            "tools": len(request.get("tools") or []),
            "last_role": last.get("role"),
            "last_text_bytes": len(json.dumps(last.get("content"), ensure_ascii=False) or ""),
        }
        path = self._stub().log_path
        if path:
            with open(path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(line, ensure_ascii=False) + "\n")


def main():
    parser = argparse.ArgumentParser(description="Celira 验收桩：脚本化的 chat completions 端点")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--script", required=True, help="脚本 JSON（数组）")
    parser.add_argument("--log", default=None, help="请求读数落盘（ndjson）")
    options = parser.parse_args()

    with open(options.script, encoding="utf-8") as handle:
        entries = json.load(handle)
    if not isinstance(entries, list):
        print("[stub] 脚本必须是数组", file=sys.stderr)
        return 2

    server = StubServer(("127.0.0.1", options.port), Handler, Script(entries), options.log)
    print(f"[stub] 在 127.0.0.1:{options.port} 上发牌：{len(entries)} 条脚本条目", flush=True)

    def stop(_signum, _frame):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        server.server_close()
        print("[stub] 收场", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
