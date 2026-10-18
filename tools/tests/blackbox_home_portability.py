#!/usr/bin/env python3
"""Home Portability 真实进程黑盒（第一层 L0/L1 保真 Disaster Restore）。

只使用公开 Core 公共协议 + 本工具 CLI 证明 backup→verify→restore：
1. 真实 celira-im 启动在临时 db + media root；bootstrap-human 取 Human Session
   （token 只留在进程内存，绝不打印）。
2. 公开 API 创建 Agent + Direct Room，multipart 上传附件并绑定 Message
   （真实 attachments 字节落到 `<media_root>/attachments/`），下载复验原始字节。
3. 作者撤回前签发第二 Human Session（模拟配对设备已预签名）与 Pairing Grant 兑换。
4. 干净停止 -> 本工具 backup -> verify -> restore 到新目录。
5. 用恢复后的 core.db + media 启动新 Core：GET /health、/ready 匿名 200；
   带「恢复前已签发」的旧 Human Session 调用 GET /v1/home，证明 home_id 不变、
   Session 仍可用；再用旧 Session 下载附件字节与原始一致。
6. 全程不打印任何 token/hash/正文；无凭据写入任何文件。

用法：
  python3 tools/tests/blackbox_home_portability.py [CELIRA_IM_BIN]
  # 默认二进制：<repo>/target/release/celira-im
退出码：0 = 全部 PASS；非零 = FAIL。
"""

from __future__ import annotations

import http.client
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import uuid as uuid_mod

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
TOOL = os.path.join(REPO, "tools", "home_portability.py")
DEFAULT_BIN = os.path.join(REPO, "target", "release", "celira-im")
UUID4_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")


class Http:
    def __init__(self, host: str, port: int):
        self.host, self.port = host, port

    def request(self, method: str, path: str, *, token: str | None = None,
                idem: str | None = None, content_type: str | None = None,
                body: bytes | None = None) -> tuple[int, dict, bytes]:
        conn = http.client.HTTPConnection(self.host, self.port, timeout=10)
        headers = {}
        if token is not None:
            headers["Authorization"] = f"Bearer {token}"
        if idem is not None:
            headers["Idempotency-Key"] = idem
        if content_type is not None:
            headers["Content-Type"] = content_type
        if body is not None:
            headers["Content-Length"] = str(len(body))
        conn.request(method, path, body=body, headers=headers)
        resp = conn.getresponse()
        raw = resp.read()
        status = resp.status
        try:
            js = json.loads(raw.decode("utf-8")) if raw else {}
        except ValueError:
            js = {}
        conn.close()
        return status, js, raw


def spawn_core(bin_path: str, db_path: str, media_root: str):
    listener = ("127.0.0.1", None)
    import socket
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    addr = f"127.0.0.1:{port}"
    proc = subprocess.Popen(
        [bin_path],
        env={
            **os.environ,
            "CELIRA_LISTEN_ADDR": addr,
            "CELIRA_DB_PATH": db_path,
            "CELIRA_MEDIA_ROOT": media_root,
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    return proc, addr, listener


def wait_ready(addr: str, timeout_s: float = 30) -> None:
    host, port = addr.split(":")
    http = Http(host, int(port))
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            status, _, _ = http.request("GET", "/ready")
            if status == 200:
                return
        except OSError:
            pass
        time.sleep(0.2)
    raise TimeoutError(f"core did not become ready at {addr}")


def stop_core(proc: subprocess.Popen) -> None:
    os.killpg(os.getpgid(proc.pid), 15)  # SIGTERM 整组
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(os.getpgid(proc.pid), 9)
        proc.wait(timeout=5)


def cli_json(bin_path: str, db_path: str, args: list[str]):
    out = subprocess.run(
        [bin_path, *args],
        env={**os.environ, "CELIRA_DB_PATH": db_path},
        capture_output=True,
        text=True,
    )
    assert out.returncode == 0, f"CLI {args} failed: {out.stderr}"
    return json.loads(out.stdout)


def multipart(file_name: str, content_type: str, data: bytes) -> tuple[str, bytes]:
    boundary = "----celira-portability-blackbox"
    parts = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{file_name}"\r\n'
        f"Content-Type: {content_type}\r\n\r\n"
    ).encode()
    return f"multipart/form-data; boundary={boundary}", parts + data + f"\r\n--{boundary}--\r\n".encode()


def run_tool(*argv):
    out = subprocess.run([sys.executable, TOOL, *argv], capture_output=True, text=True)
    return out


def main() -> int:
    bin_path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_BIN
    if not os.path.isfile(bin_path):
        print(f"FATAL: celira-im binary not found: {bin_path}")
        return 2

    temp = tempfile.TemporaryDirectory(prefix="cp-blackbox-")
    base = temp.name
    db1 = os.path.join(base, "phase1", "core.db")
    media1 = os.path.join(base, "phase1", "media")
    os.makedirs(os.path.dirname(db1))
    os.makedirs(os.path.join(media1, "attachments"))
    restore_dir = os.path.join(base, "phase2")
    backup_dir = os.path.join(base, "backup")
    steps: list[tuple[str, str]] = []

    def step(name: str, ok: bool, detail: str = "") -> None:
        steps.append((name, "PASS" if ok else "FAIL" + (f": {detail}" if detail else "")))

    try:
        proc1, addr1, _ = spawn_core(bin_path, db1, media1)
        wait_ready(addr1)
        http1 = Http("127.0.0.1", int(addr1.split(":")[1]))

        owner = cli_json(bin_path, db1, ["bootstrap-human", "--display-name", "Owner"])
        owner_token = owner["token"]  # 只在内存，绝不打印
        home0 = http1.request("GET", "/v1/home", token=owner_token)
        step("phase1: GET /v1/home (pre-restore)", home0[0] == 200)
        home_id = home0[1]["home_id"]
        step("phase1: home_id is UUID v4", bool(UUID4_RE.match(home_id)))

        # 公开 API：Agent + Direct Room。
        status, js, _ = http1.request(
            "POST", "/v1/agents", token=owner_token,
            idem=f"bb-ag-{uuid_mod.uuid4()}", content_type="application/json",
            body=json.dumps({"display_name": "Blackbox Agent"}).encode())
        step("phase1: create agent (public API)", status == 201 and "agent" in js)
        agent_id = js["agent"]["id"] if status == 201 else None

        status, js, _ = http1.request(
            "POST", "/v1/direct-rooms", token=owner_token,
            idem=f"bb-room-{uuid_mod.uuid4()}", content_type="application/json",
            body=json.dumps({"agent_id": agent_id}).encode())
        step("phase1: create direct room (public API)", status == 201 and "room" in js)
        room_id = js["room"]["id"] if status == 201 else None

        # 第二 Human Session（模拟配对设备预签名 Session）。
        session2 = cli_json(bin_path, db1, ["issue-human-session", "--human-id", owner["participant_id"]])
        session2_token = session2["token"]  # 只在内存

        # Backend Binding + Credential（公开 API）：证明 Backend 摘要随行。
        status, js, _ = http1.request(
            "POST", f"/v1/agents/{agent_id}/backend-bindings", token=owner_token,
            idem=f"bb-bind-{uuid_mod.uuid4()}", content_type="application/json",
            body=json.dumps({"label": "Blackbox Backend"}).encode())
        step("phase1: create backend binding (public API)", status == 201 and "binding" in js)
        binding_id = js["binding"]["id"] if status == 201 else None
        status, js, _ = http1.request(
            "POST", f"/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials",
            token=owner_token, idem=f"bb-cred-{uuid_mod.uuid4()}",
            content_type="application/json",
            body=json.dumps({"replaces_credential_id": None}).encode())
        step("phase1: create backend credential (public API)",
             status == 201 and "credential" in js and "token" in js)
        backend_token = js.get("token")  # 只在内存

        # Pairing Grant（operator CLI）：证明 Pairing 摘要随行，恢复后可兑换。
        grant = cli_json(bin_path, db1, [
            "issue-pairing-grant", "--human-id", owner["participant_id"],
            "--device-label", "pre-restore-device"])
        grant_token = grant["token"]  # 只在内存

        # 附件上传 + 绑定 Message。
        payload = b"celira home portability attachment payload \x00\xff binary"
        ctype, mbody = multipart("note.bin", "application/octet-stream", payload)
        status, js, _ = http1.request(
            "POST", f"/v1/rooms/{room_id}/attachments", token=owner_token,
            content_type=ctype, body=mbody)
        step("phase1: upload attachment (multipart public API)", status == 201 and "attachment" in js)
        attachment_id = js["attachment"]["id"] if status == 201 else None

        status, js, _ = http1.request(
            "POST", f"/v1/rooms/{room_id}/messages", token=owner_token,
            idem=f"bb-msg-{uuid_mod.uuid4()}", content_type="application/json",
            body=json.dumps({"attachments": [attachment_id], "body": "with attachment"}).encode())
        step("phase1: bind attachment into a message", status == 201)

        status, _, raw = http1.request(
            "GET", f"/v1/rooms/{room_id}/attachments/{attachment_id}", token=owner_token)
        step("phase1: download attachment matches bytes", status == 200 and raw == payload)

        # 干净停止后确认无 -wal/-shm 残留（backup 对未干净停止 fail closed）。
        stop_core(proc1)
        sidecars = [n for n in os.listdir(os.path.dirname(db1)) if n.endswith(("-wal", "-shm"))]
        step("phase1: clean stop leaves no -wal/-shm", sidecars == [])
        att_files_before = sorted(os.listdir(os.path.join(media1, "attachments")))
        step("phase1: media/attachments non-empty", len(att_files_before) >= 1)

        # 工具：backup / verify / restore。
        r = run_tool("backup", "--db", db1, "--media-root", media1, "--output", backup_dir,
                     "--confirm-core-stopped")
        step("tool: backup", r.returncode == 0, r.stderr.strip())
        backup_report = json.loads(r.stdout) if r.returncode == 0 else {}
        step("tool: backup home_id stable", backup_report.get("home_id") == home_id)
        step("tool: backup reports migrations", backup_report.get("migration_count", 0) >= 1)
        step("tool: backup reports attachments", backup_report.get("attachment_count", 0) >= 1)

        r = run_tool("verify", "--backup", backup_dir)
        step("tool: verify", r.returncode == 0, r.stderr.strip())

        r = run_tool("restore", "--backup", backup_dir, "--destination", restore_dir,
                     "--confirm-core-stopped")
        step("tool: restore", r.returncode == 0, r.stderr.strip())
        step("tool: restore marker written",
             os.path.isfile(os.path.join(restore_dir, "restore-manifest.json")))

        # 恢复后的真实 Core。
        db2 = os.path.join(restore_dir, "core.db")
        media2 = os.path.join(restore_dir, "media")
        proc2, addr2, _ = spawn_core(bin_path, db2, media2)
        wait_ready(addr2)
        http2 = Http("127.0.0.1", int(addr2.split(":")[1]))
        status, _, _ = http2.request("GET", "/health")
        step("phase2: GET /health 200 (anonymous)", status == 200)
        status, _, _ = http2.request("GET", "/ready")
        step("phase2: GET /ready 200 (anonymous)", status == 200)

        status, js, _ = http2.request("GET", "/v1/home", token=owner_token)
        step("phase2: old Session GET /v1/home 200 (Session survives)",
             status == 200 and js.get("home_id") == home_id)

        status, js, _ = http2.request("GET", "/v1/home", token=session2_token)
        step("phase2: second pre-restore Session survives",
             status == 200 and js.get("home_id") == home_id)

        status, _, raw = http2.request(
            "GET", f"/v1/rooms/{room_id}/attachments/{attachment_id}", token=owner_token)
        step("phase2: attachment downloadable with old Session, bytes identical",
             status == 200 and raw == payload)
        att_files_after = sorted(os.listdir(os.path.join(media2, "attachments")))
        step("phase2: restore copied whole attachments namespace",
             att_files_after == att_files_before)

        # Pairing Grant 在恢复后的 Core 上仍可兑换（Pairing 摘要随行）。
        status, js, _ = http2.request(
            "POST", "/v1/home/pairing-grants/exchange", token=grant_token,
            content_type="application/json",
            body=json.dumps({"device_label": "post-restore-device"}).encode())
        step("phase2: pre-restore Pairing Grant still exchangeable",
             status == 201 and js.get("home", {}).get("home_id") == home_id)
        exchanged_token = js.get("token") if status == 201 else None
        status, js, _ = http2.request("GET", "/v1/home", token=exchanged_token)
        step("phase2: exchanged (post-restore) Session works",
             status == 200 and js.get("home_id") == home_id)

        # Backend Credential 在恢复后的 Core 上仍可认证（Backend binding 摘要随行）。
        status, _, _ = http2.request(
            "GET", "/v1/backend/events?after=c1.be.ack&limit=100", token=backend_token)
        step("phase2: pre-restore Backend Credential still authenticates", status == 200)

        stop_core(proc2)

        # 失败路径抽检（真实 Core 库）：
        # 未确认 stopped -> backup 拒绝。
        r = run_tool("backup", "--db", db1, "--media-root", media1, "--output",
                     os.path.join(base, "bk2"))
        ok_confirm = r.returncode == 2
        step("tool: backup without confirm fails (exit 2)", ok_confirm)
        # verify 对篡改附件非零。
        tampered = os.path.join(backup_dir, "media", "attachments", att_files_before[0])
        with open(tampered, "r+b") as f:
            head = f.read(1)
            f.seek(0)
            f.write(bytes([head[0] ^ 0xFF]))
        r = run_tool("verify", "--backup", backup_dir)
        step("tool: verify fails on tampered attachment (non-zero)", r.returncode != 0)

        for name, result in steps:
            print(f"{result}  {name}")
        failed = [s for s in steps if not s[1].startswith("PASS")]
        print()
        print(f"RESULT: {'PASS' if not failed else 'FAIL'}  ({len(steps) - len(failed)}/{len(steps)} steps)")
        print(f"home_id: {home_id}")
        print(f"migration_count: {backup_report.get('migration_count')}")
        print(f"attachment_count: {backup_report.get('attachment_count')} bytes={backup_report.get('attachment_bytes')}")
        print(f"backup: {backup_dir}")
        print(f"restored: {restore_dir}")
        return 0 if not failed else 1
    except Exception as exc:  # noqa: BLE001 - 失败路径聚合报告
        print(f"FATAL: {type(exc).__name__}: {exc}")
        for name, result in steps:
            print(f"{result}  {name}")
        return 1
    finally:
        temp.cleanup()


if __name__ == "__main__":
    sys.exit(main())
