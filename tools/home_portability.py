#!/usr/bin/env python3
"""Celira Home Portability：离线（停机）Home backup / verify / restore（第一层：保真 Disaster Restore）。

权威设计：HOME_PORTABILITY.md；只使用 Python 3 标准库（sqlite3、hashlib、json、os、shutil 等）。

范围（本工具唯一事实边界）：
- 离线搬运 Home 的持久公共事实 = `core.db`（SQLite 公共事实，原样保留 token_hash 等摘要，
  使旧 Human Session / Pairing Grant 状态 / Backend Binding 摘要语义随行）+
  `<media_root>/attachments/` 受控 UUID 字节命名空间。
- 不连接 Core HTTP、不启动 Core、不读 Runtime / Client Host / Provider 凭据、不复制任何
  raw token、不输出 token/hash/正文，也不把 manifest 中的 checksum 原文（即应用迁移的
  checksum BLOB）写入 manifest。

子命令：
  backup   --db PATH --media-root PATH --output PATH --confirm-core-stopped
  verify   --backup PATH
  restore  --backup PATH --destination PATH --confirm-core-stopped
  inspect  --backup PATH

安全边界（fail closed）：
- `--confirm-core-stopped` 只承认操作者的显式确认；它不提供、也不声称提供任何双 Active Core
  检测/fencing（Home Identity 不是 fencing）。
- 拒绝绝对路径、`..`、symlink、非普通文件、越出备份根；attachments 命名空间只接受严格
  UUID v4 文件与 Core 自有临时名（`<uuid>.tmp` / `<uuid>.tmp.<uuid>`），其余名字一律失败、
  绝不静默丢弃。
- backup/restore 输出的目录 0700、文件 0600（显式设置，不依赖 umask）；写入原子（同目录
  临时文件 + fsync + rename + 父目录 fsync，best-effort）。
- verify 绝不修改备份；SQLite 以只读打开。
- restore 目标必须不存在或是空目录；绝不覆盖已有文件/目录、绝不 merge；先在 sibling 临时
  目录完整恢复并复验，成功后再原子 rename。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sqlite3
import stat
import sys
import tempfile
import uuid as uuid_mod
from datetime import datetime, timezone

FORMAT_VERSION = 1
DB_FILE = "core.db"
MANIFEST_FILE = "manifest.json"
RESTORE_MANIFEST_FILE = "restore-manifest.json"
MEDIA_SUBDIR = "attachments"
ATTACH_REL_PREFIX = f"media/{MEDIA_SUBDIR}"

# Core 附件 storage key 必须是规范小写连字符 UUID v4（CORE_V1_IM_CONTRACT.md §8；
# im/server/src/adapters/blob.rs 只认严格 UUID v4 + Core 自有临时名）。
_UUID_V4_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)

# Core 自己的临时文件命名（blob.rs sweep_name）：<uuid>.tmp 与 <uuid>.tmp.<uuid>。
_TMP_RE = re.compile(r"^([0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})\.tmp(\.[0-9a-f-]{36})?$")
_TMP_UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")

EXIT_OK = 0
EXIT_OP = 1  # 操作失败（校验/完整性/文件系统/权限/秘密等）
EXIT_USAGE = 2  # 参数/用法错误


class PortabilityError(Exception):
    """稳定分类的错误。reason 只含分类，detail 可留 file 级信息但不含秘密。"""

    def __init__(self, reason: str, detail: str = "") -> None:
        super().__init__(detail or reason)
        self.reason = reason
        self.detail = detail

    def __str__(self) -> str:  # pragma: no cover - trivial
        return self.detail if self.detail else self.reason


def _die(reason: str, detail: str = "", exit_code: int = EXIT_OP) -> "None":
    if detail:
        print(f"error: {reason}: {detail}", file=sys.stderr)
    else:
        print(f"error: {reason}", file=sys.stderr)
    sys.exit(exit_code)


def _usage_error(message: str) -> "None":
    print(f"error: {message}", file=sys.stderr)
    sys.exit(EXIT_USAGE)


def _is_uuid4(name: str) -> bool:
    return bool(_UUID_V4_RE.match(name))


def _is_core_tmp_name(name: str) -> bool:
    m = _TMP_RE.match(name)
    if not m:
        return False
    nonce = m.group(2)
    return nonce is None or _TMP_UUID_RE.match(nonce[1:]) is not None


def _lstat_regular(path: str, what: str) -> os.stat_result:
    try:
        st = os.lstat(path)
    except FileNotFoundError as e:
        raise PortabilityError(f"{what}_missing", f"{what} not found: {path}") from e
    except OSError as e:
        raise PortabilityError(f"{what}_stat_failed", f"cannot stat {what}: {e}") from e
    if stat.S_ISLNK(st.st_mode):
        raise PortabilityError(f"{what}_symlink", f"{what} must not be a symlink: {path}")
    if not stat.S_ISREG(st.st_mode):
        raise PortabilityError(f"{what}_not_regular", f"{what} must be a regular file: {path}")
    return st


def _lstat_dir(path: str, what: str) -> os.stat_result:
    try:
        st = os.lstat(path)
    except FileNotFoundError as e:
        raise PortabilityError(f"{what}_missing", f"{what} not found: {path}") from e
    except OSError as e:
        raise PortabilityError(f"{what}_stat_failed", f"cannot stat {what}: {e}") from e
    if stat.S_ISLNK(st.st_mode):
        raise PortabilityError(f"{what}_symlink", f"{what} must not be a symlink: {path}")
    if not stat.S_ISDIR(st.st_mode):
        raise PortabilityError(f"{what}_not_dir", f"{what} must be a directory: {path}")
    return st


def _fsync_dir(path: str) -> None:
    # Linux/Unix 目录项持久化的 best-effort；不保证跨平台。
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(fd)
    except OSError:
        pass
    finally:
        os.close(fd)


def _atomic_write_bytes(path: str, data: bytes) -> None:
    """同目录唯一临时名 + O_EXCL + fsync + rename + 父目录 fsync；文件显式 0600。"""
    directory = os.path.dirname(path)
    name = os.path.basename(path)
    tmp = os.path.join(directory, f".{name}.tmp.{os.getpid()}.{uuid_mod.uuid4().hex}")
    fd = None
    try:
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
        os.chmod(path, 0o600)
        _fsync_dir(directory)
    except OSError as e:
        raise PortabilityError("backup_write_failed", f"cannot write {path}: {e}") from e
    finally:
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass
        try:
            if os.path.exists(tmp):
                os.unlink(tmp)
        except OSError:
            pass


def _copy_stream_hashed(src: str, dst: str, chunk: int = 1 << 16) -> tuple[int, str]:
    """流式拷贝文件，同时计算 size 与 sha256；目标文件显式 0600，写入原子。"""
    directory = os.path.dirname(dst)
    name = os.path.basename(dst)
    tmp = os.path.join(directory, f".{name}.tmp.{os.getpid()}.{uuid_mod.uuid4().hex}")
    hasher = hashlib.sha256()
    size = 0
    try:
        with open(src, "rb") as fin:
            with os.fdopen(
                os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb"
            ) as fout:
                while True:
                    block = fin.read(chunk)
                    if not block:
                        break
                    fout.write(block)
                    hasher.update(block)
                    size += len(block)
                fout.flush()
                os.fsync(fout.fileno())
        os.chmod(tmp, 0o600)
        os.replace(tmp, dst)
        os.chmod(dst, 0o600)
        _fsync_dir(directory)
    except OSError as e:
        raise PortabilityError("backup_write_failed", f"cannot copy {src} -> {dst}: {e}") from e
    finally:
        try:
            if os.path.exists(tmp):
                os.unlink(tmp)
        except OSError:
            pass
    return size, hasher.hexdigest()


def _file_sha256(path: str, chunk: int = 1 << 16) -> tuple[int, str]:
    hasher = hashlib.sha256()
    size = 0
    with open(path, "rb") as f:
        while True:
            block = f.read(chunk)
            if not block:
                break
            hasher.update(block)
            size += len(block)
    return size, hasher.hexdigest()


def _open_db_readonly(db_path: str) -> sqlite3.Connection:
    absolute = os.path.abspath(db_path)
    try:
        conn = sqlite3.connect(f"file:{absolute}?mode=ro", uri=True)
    except sqlite3.Error as e:
        raise PortabilityError("db_open_failed", f"cannot open SQLite read-only: {e}") from e
    try:
        conn.execute("PRAGMA query_only = ON")
        conn.execute("PRAGMA foreign_keys = ON")
    except sqlite3.Error as e:
        conn.close()
        raise PortabilityError("db_open_failed", f"cannot prepare SQLite read-only: {e}") from e
    return conn


# ---------------------------------------------------------------------------
# SQLite 事实读取（只读）
# ---------------------------------------------------------------------------

def _db_checks(db_path: str) -> None:
    """备份/校验共用的 SQLite 完整性事实。失败即非零。"""
    _lstat_regular(db_path, "database")
    conn = _open_db_readonly(db_path)
    try:
        rows = conn.execute("PRAGMA integrity_check").fetchall()
        if not rows or any(str(r[0]).strip().lower() != "ok" for r in rows):
            raise PortabilityError(
                "db_integrity_check_failed",
                f"PRAGMA integrity_check returned anomalies in {db_path}",
            )
        fk = conn.execute("PRAGMA foreign_key_check").fetchall()
        if fk:
            raise PortabilityError(
                "db_foreign_key_check_failed",
                f"PRAGMA foreign_key_check returned {len(fk)} violation(s)",
            )
    finally:
        conn.close()


def _db_home(conn: sqlite3.Connection) -> dict:
    try:
        rows = conn.execute(
            "SELECT home_id, display_name FROM home WHERE id = 1"
        ).fetchall()
    except sqlite3.Error as e:
        raise PortabilityError("db_home_read_failed", f"cannot read home table: {e}") from e
    if len(rows) != 1:
        raise PortabilityError(
            "db_home_singleton_missing",
            f"expected exactly one home row, found {len(rows)}",
        )
    home_id, display_name = rows[0]
    if not isinstance(home_id, str) or not _is_uuid4(home_id):
        raise PortabilityError("db_home_id_invalid", "home_id is not a canonical UUID v4")
    return {"home_id": home_id, "home_display_name": display_name}


def _db_migrations(conn: sqlite3.Connection) -> list[dict]:
    try:
        rows = conn.execute(
            "SELECT version, description, success FROM _sqlx_migrations ORDER BY version"
        ).fetchall()
    except sqlite3.Error as e:
        raise PortabilityError("db_migrations_read_failed", f"cannot read _sqlx_migrations: {e}") from e
    applied = []
    for version, description, success in rows:
        ok = bool(success)
        if not ok:
            raise PortabilityError(
                "db_migration_failed",
                f"migration version {version} is marked failed (success=0)",
            )
        applied.append({"version": int(version), "description": str(description), "success": ok})
    return applied


def _db_has_sidecar_journals(db_path: str) -> str:
    """库旁边还留着**日志兄弟文件**吗（留着一个就说明 Core 没干净停机）。

    ⚠ 2026-09-25 修正：原来只查 `-wal` / `-shm`（WAL 模式的兄弟），而**本构建用的是
    SQLite 默认的 rollback journal**（`im/server/src` 里没有任何 `journal_mode` pragma
    ⇒ 默认 DELETE 模式）⇒ 崩溃/硬停留下的是 **`core.db-journal`**。
    只查 WAL 那两个后缀，等于这道 fail-closed **永不触发**（真机上 `im/server/data/*/`
    里就躺着 `core.db-journal`）。三种都查：它们都表示"库可能处在半写状态"。
    """
    for suffix in ("-wal", "-shm", "-journal"):
        if os.path.exists(db_path + suffix):
            return suffix
    return ""


# ---------------------------------------------------------------------------
# attachments 命名空间扫描与安全校验
# ---------------------------------------------------------------------------

def _classify_attachment_name(name: str) -> str:
    """返回 'final' / 'temp' / None。None = 非 Core 命名空间名字（拒绝，不静默丢弃）。"""
    if _is_uuid4(name):
        return "final"
    if _is_core_tmp_name(name):
        return "temp"
    return None


def _scan_attachments(attachments_dir: str) -> list[dict]:
    """枚举 `<media_root>/attachments` 的单层受控命名空间；任一越界/未知/非普通文件即失败。

    Core 不会在 attachments 下创建子目录；未知名字/子目录/符号链接一律显式失败，
    绝不停默丢弃。返回条目：{name, size, sha256}，保持字典序（确定性 manifest）。
    """
    _lstat_dir(attachments_dir, "attachments")
    names = sorted(os.listdir(attachments_dir))
    entries = []
    for name in names:
        kind = _classify_attachment_name(name)
        if kind is None:
            raise PortabilityError(
                "attachment_unknown_name",
                f"attachments namespace contains unsupported entry {name!r}; refusing to backup",
            )
        path = os.path.join(attachments_dir, name)
        st = _lstat_regular(path, "attachment")
        size, digest = _file_sha256(path)
        if size != st.st_size:
            raise PortabilityError(
                "attachment_changed_during_backup",
                f"attachment {name!r} changed while reading",
            )
        entries.append(
            {
                "relative_path": f"{ATTACH_REL_PREFIX}/{name}",
                "name": name,
                "size": size,
                "sha256": digest,
            }
        )
    return entries


# ---------------------------------------------------------------------------
# manifest 构造与校验
# ---------------------------------------------------------------------------

def _build_manifest(db_path: str, home: dict, applied: list[dict], db_size: int,
                    db_digest: str, attachments: list[dict]) -> dict:
    return {
        "format_version": FORMAT_VERSION,
        "tool": "tools/home_portability.py",
        "home_id": home["home_id"],
        "home_display_name": home["home_display_name"],
        "created_at": datetime.now(timezone.utc).isoformat(),
        "applied_migrations": applied,
        "core.db": {"size": db_size, "sha256": db_digest},
        "attachment_count": len(attachments),
        "attachment_bytes": sum(int(a["size"]) for a in attachments),
        "attachments": [
            {"relative_path": a["relative_path"], "size": int(a["size"]), "sha256": a["sha256"]}
            for a in attachments
        ],
    }


# path 安全：相对、无 ..、无绝对路径、仅 media/attachments/<单段名>。
def _safe_attach_rel(rel: str) -> str:
    if not isinstance(rel, str):
        raise PortabilityError("manifest_invalid", "attachment relative_path must be a string")
    if "\x00" in rel:
        raise PortabilityError("manifest_invalid_path", "attachment relative_path contains NUL")
    if rel.startswith("/") or rel.startswith("\\"):
        raise PortabilityError("manifest_absolute_path", f"absolute path in manifest: {rel!r}")
    parts = rel.split("/")
    if any(p in ("", ".", "..") for p in parts) or any("\\" in p for p in parts):
        raise PortabilityError(
            "manifest_unsafe_path", f"unsafe attachment relative path in manifest: {rel!r}"
        )
    if len(parts) != 3 or parts[0] != "media" or parts[1] != MEDIA_SUBDIR:
        raise PortabilityError(
            "manifest_unsafe_path",
            f"attachment relative path must be exactly media/attachments/<name>: {rel!r}",
        )
    name = parts[2]
    if _classify_attachment_name(name) is None:
        raise PortabilityError(
            "manifest_unknown_attachment_name",
            f"manifest references non-Core attachment name: {name!r}",
        )
    return rel


def _expect_str(m: dict, key: str) -> str:
    v = m.get(key)
    if not isinstance(v, str):
        raise PortabilityError("manifest_invalid", f"manifest field {key!r} must be a string")
    return v


def _expect_int(m: dict, key: str) -> int:
    v = m.get(key)
    if isinstance(v, bool) or not isinstance(v, int):
        raise PortabilityError("manifest_invalid", f"manifest field {key!r} must be an integer")
    return v


def _expect_nonneg_int(m: dict, key: str) -> int:
    v = _expect_int(m, key)
    if v < 0:
        raise PortabilityError("manifest_invalid", f"manifest field {key!r} must be a non-negative integer")
    return v


def _read_manifest(backup_dir: str) -> dict:
    manifest_path = os.path.join(backup_dir, MANIFEST_FILE)
    _lstat_regular(manifest_path, "manifest")
    try:
        with open(manifest_path, "r", encoding="utf-8") as f:
            m = json.load(f)
    except (OSError, ValueError) as e:
        raise PortabilityError("manifest_unreadable", f"cannot parse manifest.json: {e}") from e
    if not isinstance(m, dict):
        raise PortabilityError("manifest_invalid", "manifest root must be an object")
    allowed = {
        "format_version", "tool", "home_id", "home_display_name", "created_at",
        "applied_migrations", "core.db", "attachment_count", "attachment_bytes",
        "attachments",
    }
    unknown = set(m.keys()) - allowed
    if unknown:
        raise PortabilityError(
            "manifest_unknown_fields", f"manifest has unknown fields: {sorted(unknown)!r}"
        )
    if m.get("format_version") != FORMAT_VERSION:
        raise PortabilityError(
            "manifest_unsupported_version",
            f"unsupported manifest format_version: {m.get('format_version')!r}",
        )
    home_id = _expect_str(m, "home_id")
    if not _is_uuid4(home_id):
        raise PortabilityError("manifest_invalid", "manifest home_id is not a canonical UUID v4")
    display = m.get("home_display_name")
    if display is not None and not isinstance(display, str):
        raise PortabilityError("manifest_invalid", "home_display_name must be a string or null")
    _expect_str(m, "created_at")
    if _expect_str(m, "tool") != "tools/home_portability.py":
        raise PortabilityError(
            "manifest_tool_mismatch", f"unsupported manifest tool: {m['tool']!r}"
        )
    core = m.get("core.db")
    if not isinstance(core, dict) or set(core.keys()) != {"size", "sha256"}:
        raise PortabilityError("manifest_invalid", "manifest core.db must be {{size, sha256}}")
    _expect_int(core, "size")
    sha = _expect_str(core, "sha256")
    if not re.fullmatch(r"[0-9a-f]{64}", sha):
        raise PortabilityError("manifest_invalid", "core.db sha256 must be 64 hex chars")
    migs = m.get("applied_migrations")
    if not isinstance(migs, list):
        raise PortabilityError("manifest_invalid", "applied_migrations must be an array")
    for item in migs:
        if not isinstance(item, dict) or set(item.keys()) != {"version", "description", "success"}:
            raise PortabilityError("manifest_invalid", "migration entry must be {{version, description, success}}")
        _expect_int(item, "version")
        _expect_str(item, "description")
        if item.get("success") is not True:
            raise PortabilityError("manifest_migration_failed", "manifest lists a failed migration")
    atts = m.get("attachments")
    if not isinstance(atts, list):
        raise PortabilityError("manifest_invalid", "attachments must be an array")
    seen = set()
    for a in atts:
        if not isinstance(a, dict) or set(a.keys()) != {"relative_path", "size", "sha256"}:
            raise PortabilityError("manifest_invalid", "attachment entry must be {{relative_path,size,sha256}}")
        rel = _safe_attach_rel(_expect_str(a, "relative_path"))
        _expect_int(a, "size")
        ash = _expect_str(a, "sha256")
        if not re.fullmatch(r"[0-9a-f]{64}", ash):
            raise PortabilityError("manifest_invalid", "attachment sha256 must be 64 hex chars")
        if rel in seen:
            raise PortabilityError("manifest_duplicate_attachment", f"duplicate attachment path {rel!r}")
        seen.add(rel)
    count = _expect_nonneg_int(m, "attachment_count")
    total = _expect_nonneg_int(m, "attachment_bytes")
    if count != len(atts):
        raise PortabilityError(
            "manifest_count_mismatch",
            f"manifest attachment_count {count} != attachments array length {len(atts)}",
        )
    expected_sum = sum(int(a["size"]) for a in atts)
    if total != expected_sum:
        raise PortabilityError(
            "manifest_bytes_mismatch",
            f"manifest attachment_bytes {total} != sum of attachment sizes {expected_sum}",
        )
    return m


# ---------------------------------------------------------------------------
# backup
# ---------------------------------------------------------------------------

def _cmd_backup(args: argparse.Namespace) -> int:
    if not args.confirm_core_stopped:
        _usage_error("backup requires --confirm-core-stopped (Operator confirms the Home Core is stopped)")
    db_path = os.path.abspath(args.db)
    media_root = os.path.abspath(args.media_root)
    output = os.path.abspath(args.output)

    _lstat_regular(db_path, "database")
    sidecar = _db_has_sidecar_journals(db_path)
    if sidecar:
        _die(
            "db_unclean_stop",
            f"found {db_path}{sidecar}: SQLite journals not checkpointed; Core did not stop cleanly. "
            "Stop the Core cleanly, restart once, and retry.",
        )
    _lstat_dir(media_root, "media_root")
    attachments_dir = os.path.join(media_root, MEDIA_SUBDIR)
    _lstat_dir(attachments_dir, "attachments")

    if os.path.lexists(output):
        _die("output_exists", f"output already exists: {output}")
    out_parent = os.path.dirname(output)
    if not os.path.isdir(out_parent) or os.path.islink(out_parent):
        _die("output_parent_invalid", f"output parent must be an existing directory: {out_parent}")
    # 防自嵌套：备份输出不能落在媒体根或数据库目录之内（否则会扫描到自己）。
    for base in (media_root, os.path.dirname(db_path)):
        try:
            common = os.path.commonpath([output, base])
            if common in (output, base):
                _die("output_inside_source", f"output must not be inside {base}")
        except ValueError:
            pass

    # 先跑 SQLite 完整性（fail fast，避免把坏库做成备份）。
    _db_checks(db_path)
    conn = _open_db_readonly(db_path)
    try:
        home = _db_home(conn)
        applied = _db_migrations(conn)
    finally:
        conn.close()

    db_size, db_digest = _file_sha256(db_path)
    entries = _scan_attachments(attachments_dir)

    manifest = _build_manifest(
        db_path, home, applied, db_size, db_digest,
        [{"relative_path": e["relative_path"], "size": e["size"], "sha256": e["sha256"]} for e in entries],
    )

    # 在输出目录内构建（输出目录 0700）；任何失败清理半成品并退出非零。
    media_out = os.path.join(output, "media", MEDIA_SUBDIR)
    try:
        os.mkdir(output, 0o700)
        os.chmod(output, 0o700)
        media_dir = os.path.join(output, "media")
        os.mkdir(media_dir, 0o700)
        os.chmod(media_dir, 0o700)
        os.mkdir(media_out, 0o700)
        os.chmod(media_out, 0o700)
        # core.db 先写（manifest 在最后）；逐项比对拷贝结果与预读值，不一致即失败并清理。
        db_size2, db_digest2 = _copy_stream_hashed(db_path, os.path.join(output, DB_FILE))
        if db_size2 != db_size or db_digest2 != db_digest:
            raise PortabilityError(
                "database_changed_during_backup", "core.db changed while it was being copied"
            )
        for e in entries:
            src = os.path.join(attachments_dir, e["name"])
            size2, digest2 = _copy_stream_hashed(src, os.path.join(media_out, e["name"]))
            if size2 != e["size"] or digest2 != e["sha256"]:
                raise PortabilityError(
                    "attachment_changed_during_backup",
                    f"attachment {e['name']!r} changed while it was being copied",
                )
        manifest_bytes = json.dumps(manifest, indent=2, ensure_ascii=False).encode("utf-8") + b"\n"
        _atomic_write_bytes(os.path.join(output, MANIFEST_FILE), manifest_bytes)
    except PortabilityError:
        shutil.rmtree(output, ignore_errors=True)
        raise
    except OSError as e:
        shutil.rmtree(output, ignore_errors=True)
        _die("backup_failed", str(e))

    _report(
        "backup created",
        {
            "output": output,
            "home_id": home["home_id"],
            "home_display_name": home["home_display_name"],
            "migration_count": len(applied),
            "migration_max_version": applied[-1]["version"] if applied else None,
            "attachment_count": manifest["attachment_count"],
            "attachment_bytes": manifest["attachment_bytes"],
            "db_size": db_size,
        },
    )
    return EXIT_OK


# ---------------------------------------------------------------------------
# verify
# ---------------------------------------------------------------------------

def _listdir(path: str, what: str) -> list[str]:
    try:
        return sorted(os.listdir(path))
    except OSError as e:
        raise PortabilityError("backup_read_failed", f"cannot list {what}: {e}") from e


def _verify_tree_structure(resolved: str, m: dict) -> dict:
    """显式结构检查（不使用 os.walk，杜绝 symlink 目录被静默跳过）。

    - 备份根：恰好 manifest.json/core.db 两个普通文件 + media 一个非 symlink 目录，
      无其它文件/目录；media/ 恰好含 attachments 一个非 symlink 目录，无额外条目；
    - attachments 目录必须存在（即使 manifest attachments=[]），其内只允许 manifest
      列出的非 symlink 普通文件（manifest 列出的必须全部存在），不允许子目录、
      额外文件或 symlink；
    - 返回 {manifest 相对路径: 磁盘绝对路径} 映射。
    """
    root_entries = _listdir(resolved, "backup root")
    seen_root = set()
    for name in root_entries:
        p = os.path.join(resolved, name)
        st = os.lstat(p)
        if stat.S_ISLNK(st.st_mode):
            raise PortabilityError("backup_symlink", f"symlink in backup tree: {p}")
        if name in (DB_FILE, MANIFEST_FILE):
            if not stat.S_ISREG(st.st_mode):
                raise PortabilityError("backup_invalid_tree", f"backup entry must be a regular file: {p}")
        elif name == "media":
            if not stat.S_ISDIR(st.st_mode):
                raise PortabilityError("backup_invalid_tree", f"backup entry media must be a directory: {p}")
        else:
            if stat.S_ISDIR(st.st_mode):
                raise PortabilityError("backup_unexpected_dirs", f"unexpected directory at backup root: {name!r}")
            raise PortabilityError("backup_unexpected_files", f"unexpected file at backup root: {name!r}")
        seen_root.add(name)
    missing_root = {DB_FILE, MANIFEST_FILE, "media"} - seen_root
    if missing_root:
        raise PortabilityError(
            "backup_missing_files", f"backup missing entries: {sorted(missing_root)!r}"
        )

    media_dir = os.path.join(resolved, "media")
    seen_att = False
    for name in _listdir(media_dir, "media dir"):
        p = os.path.join(media_dir, name)
        st = os.lstat(p)
        if stat.S_ISLNK(st.st_mode):
            raise PortabilityError("backup_symlink", f"symlink in backup tree: {p}")
        if name != MEDIA_SUBDIR:
            if stat.S_ISDIR(st.st_mode):
                raise PortabilityError("backup_unexpected_dirs", f"unexpected directory under media/: {name!r}")
            raise PortabilityError("backup_unexpected_files", f"unexpected file under media/: {name!r}")
        if not stat.S_ISDIR(st.st_mode):
            raise PortabilityError("backup_invalid_tree", f"media/attachments must be a directory: {p}")
        seen_att = True
    if not seen_att:
        raise PortabilityError("backup_missing_files", "backup missing media/attachments directory")

    att_dir = os.path.join(media_dir, MEDIA_SUBDIR)
    expected: dict[str, dict] = {}
    for a in m["attachments"]:
        expected[a["relative_path"].rsplit("/", 1)[-1]] = a
    att_entries = _listdir(att_dir, "attachments dir")
    found: dict[str, str] = {}
    for name in att_entries:
        p = os.path.join(att_dir, name)
        st = os.lstat(p)
        if stat.S_ISLNK(st.st_mode):
            raise PortabilityError("backup_symlink", f"symlink in backup tree: {p}")
        if stat.S_ISDIR(st.st_mode):
            raise PortabilityError("backup_unexpected_dirs", f"unexpected directory in attachments: {name!r}")
        if name not in expected:
            raise PortabilityError(
                "backup_unexpected_files",
                f"backup contains files not listed in manifest: {name!r}",
            )
        if not stat.S_ISREG(st.st_mode):
            raise PortabilityError("backup_invalid_tree", f"attachment must be a regular file: {p}")
        found[expected[name]["relative_path"]] = p
    missing_att = set(expected) - set(att_entries)
    if missing_att:
        raise PortabilityError(
            "backup_missing_files",
            f"backup missing files: {sorted('media/attachments/' + n for n in missing_att)!r}",
        )
    return found


def _verify_backup(backup_dir: str) -> dict:
    """完整校验一个备份；不修改备份。返回非秘密摘要。任何异常抛 PortabilityError。"""
    resolved = os.path.abspath(backup_dir)
    _lstat_dir(resolved, "backup")

    m = _read_manifest(resolved)
    found = _verify_tree_structure(resolved, m)

    # core.db 校验：普通文件 + size + sha256；再跑 SQLite 只读完整性并核对事实。
    db_path = os.path.join(resolved, DB_FILE)
    _lstat_regular(db_path, "database")
    db_expected = m["core.db"]
    size, digest = _file_sha256(db_path)
    if size != db_expected["size"]:
        raise PortabilityError("backup_db_size_mismatch", f"core.db size mismatch: {size} != {db_expected['size']}")
    if digest != db_expected["sha256"]:
        raise PortabilityError("backup_db_hash_mismatch", "core.db sha256 mismatch")
    _db_checks(db_path)
    conn = _open_db_readonly(db_path)
    try:
        home = _db_home(conn)
        applied = _db_migrations(conn)
    finally:
        conn.close()
    if home["home_id"] != m["home_id"]:
        raise PortabilityError(
            "backup_home_mismatch",
            f"manifest home_id {m['home_id']!r} != database home_id {home['home_id']!r}",
        )
    if [x["version"] for x in applied] != [x["version"] for x in m["applied_migrations"]]:
        raise PortabilityError("backup_migrations_mismatch", "manifest migrations do not match database")
    if [x["description"] for x in applied] != [x["description"] for x in m["applied_migrations"]]:
        raise PortabilityError("backup_migrations_mismatch", "manifest migration descriptions do not match database")

    # 附件逐个校验：存在、普通文件、size、sha256；最终总字节必须与 manifest 一致。
    total_bytes = 0
    for a in m["attachments"]:
        p = found[a["relative_path"]]
        _lstat_regular(p, "attachment")
        size, digest = _file_sha256(p)
        if size != a["size"]:
            raise PortabilityError(
                "backup_attachment_size_mismatch",
                f"size mismatch for {a['relative_path']}: {size} != {a['size']}",
            )
        if digest != a["sha256"]:
            raise PortabilityError(
                "backup_attachment_hash_mismatch",
                f"sha256 mismatch for {a['relative_path']}",
            )
        total_bytes += size
    if total_bytes != m["attachment_bytes"]:
        raise PortabilityError(
            "manifest_bytes_mismatch",
            f"verified attachment total {total_bytes} != manifest attachment_bytes {m['attachment_bytes']}",
        )

    return {
        "home_id": m["home_id"],
        "home_display_name": m["home_display_name"],
        "migration_count": len(applied),
        "migration_max_version": applied[-1]["version"] if applied else None,
        "all_migrations_success": True,
        "attachment_count": len(m["attachments"]),
        "attachment_bytes": total_bytes,
    }


def _cmd_verify(args: argparse.Namespace) -> int:
    summary = _verify_backup(args.backup)
    _report("verify ok", summary)
    return EXIT_OK


def _cmd_inspect(args: argparse.Namespace) -> int:
    resolved = os.path.abspath(args.backup)
    _lstat_dir(resolved, "backup")
    m = _read_manifest(resolved)
    _report(
        "inspect",
        {
            "backup": resolved,
            "home_id": m["home_id"],
            "home_display_name": m["home_display_name"],
            "created_at": m["created_at"],
            "migration_count": len(m["applied_migrations"]),
            "migration_max_version": m["applied_migrations"][-1]["version"] if m["applied_migrations"] else None,
            "attachment_count": m["attachment_count"],
            "attachment_bytes": m["attachment_bytes"],
        },
    )
    return EXIT_OK


# ---------------------------------------------------------------------------
# restore
# ---------------------------------------------------------------------------

def _restore_into(tmp: str, resolved_backup: str, m: dict) -> None:
    """在 tmp（已 0700）内重建 core.db + media/attachments/*，并逐个复验。"""
    media_dir = os.path.join(tmp, "media")
    media_out = os.path.join(media_dir, MEDIA_SUBDIR)
    os.mkdir(media_dir, 0o700)
    os.chmod(media_dir, 0o700)
    os.mkdir(media_out, 0o700)
    os.chmod(media_out, 0o700)
    _copy_stream_hashed(os.path.join(resolved_backup, DB_FILE), os.path.join(tmp, DB_FILE))
    for a in m["attachments"]:
        name = a["relative_path"].split("/")[-1]
        _copy_stream_hashed(
            os.path.join(resolved_backup, "media", MEDIA_SUBDIR, name),
            os.path.join(media_out, name),
        )
    # 复验恢复出的文件与 manifest 一致。
    restored_db = os.path.join(tmp, DB_FILE)
    size, digest = _file_sha256(restored_db)
    if size != m["core.db"]["size"] or digest != m["core.db"]["sha256"]:
        raise PortabilityError("restore_db_mismatch", "restored core.db does not match manifest")
    for a in m["attachments"]:
        name = a["relative_path"].split("/")[-1]
        p = os.path.join(media_out, name)
        size, digest = _file_sha256(p)
        if size != a["size"] or digest != a["sha256"]:
            raise PortabilityError(
                "restore_attachment_mismatch", f"restored attachment {name!r} does not match manifest"
            )


def _cmd_restore(args: argparse.Namespace) -> int:
    if not args.confirm_core_stopped:
        _usage_error("restore requires --confirm-core-stopped (Operator confirms no Core is running on dst)")
    resolved_backup = os.path.abspath(args.backup)
    destination = os.path.abspath(args.destination)

    # 先完整 verify 备份。
    summary = _verify_backup(resolved_backup)

    dest_parent = os.path.dirname(destination)
    if not os.path.isdir(dest_parent) or os.path.islink(dest_parent):
        _die("destination_parent_invalid", f"destination parent must be an existing directory: {dest_parent}")

    if os.path.lexists(destination):
        st = os.lstat(destination)
        if stat.S_ISLNK(st.st_mode):
            _die("destination_symlink", "destination must not be a symlink")
        if not stat.S_ISDIR(st.st_mode):
            _die("destination_exists_file", "destination exists and is not a directory")
        entries = os.listdir(destination)
        if entries:
            _die("destination_not_empty", f"destination exists and is not empty: {sorted(entries)!r}")
    # destination 不存在 或 是空目录：都可在同一父目录的 sibling 临时目录构建成功后
    # 原子 rename 到位（POSIX rename 目录到空目录会替换之）。

    tmp = tempfile.mkdtemp(prefix=f".home-restore-{os.path.basename(destination)}-", dir=dest_parent)
    try:
        os.chmod(tmp, 0o700)
        _restore_into(tmp, resolved_backup, _read_manifest(resolved_backup))
        # 恢复结果标记（非秘密；绝不复制 token）。
        marker = {
            "format_version": FORMAT_VERSION,
            "tool": "tools/home_portability.py",
            "restored_at": datetime.now(timezone.utc).isoformat(),
            "source_backup": {
                "home_id": summary["home_id"],
                "home_display_name": summary["home_display_name"],
                "migration_count": summary["migration_count"],
                "migration_max_version": summary["migration_max_version"],
                "attachment_count": summary["attachment_count"],
                "attachment_bytes": summary["attachment_bytes"],
            },
            "restored_files": {
                "core.db": {"size": 0, "sha256": None},
                "media/attachments": {"count": summary["attachment_count"]},
            },
        }
        marker["restored_files"]["core.db"]["size"] = _file_sha256(os.path.join(tmp, DB_FILE))[0]
        _atomic_write_bytes(
            os.path.join(tmp, RESTORE_MANIFEST_FILE),
            json.dumps(marker, indent=2, ensure_ascii=False).encode("utf-8") + b"\n",
        )
        _fsync_dir(tmp)
        os.rename(tmp, destination)
        os.chmod(destination, 0o700)
    except PortabilityError:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    except OSError as e:
        shutil.rmtree(tmp, ignore_errors=True)
        _die("restore_failed", str(e))

    _report(
        "restore ok",
        {
            "destination": destination,
            "home_id": summary["home_id"],
            "home_display_name": summary["home_display_name"],
            "migration_count": summary["migration_count"],
            "migration_max_version": summary["migration_max_version"],
            "attachment_count": summary["attachment_count"],
            "attachment_bytes": summary["attachment_bytes"],
        },
    )
    return EXIT_OK


# ---------------------------------------------------------------------------
# 输出与入口
# ---------------------------------------------------------------------------

def _report(title: str, fields: dict) -> None:
    out = {"result": title}
    out.update(fields)
    print(json.dumps(out, ensure_ascii=False, sort_keys=True))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="home_portability", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("backup", help="create a versioned offline backup")
    p.add_argument("--db", required=True)
    p.add_argument("--media-root", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--confirm-core-stopped", action="store_true", help="Operator confirms the Home Core is stopped")
    p.set_defaults(func=_cmd_backup)

    p = sub.add_parser("verify", help="verify an offline backup without modifying it")
    p.add_argument("--backup", required=True)
    p.set_defaults(func=_cmd_verify)

    p = sub.add_parser("restore", help="verify then restore into an empty/new destination")
    p.add_argument("--backup", required=True)
    p.add_argument("--destination", required=True)
    p.add_argument("--confirm-core-stopped", action="store_true", help="Operator confirms no Core is running on destination")
    p.set_defaults(func=_cmd_restore)

    p = sub.add_parser("inspect", help="print a non-secret summary of a backup manifest")
    p.add_argument("--backup", required=True)
    p.set_defaults(func=_cmd_inspect)
    return parser


def _run(argv: list[str]) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except PortabilityError as e:
        print(f"error: {e.reason}" + (f": {e.detail}" if e.detail else ""), file=sys.stderr)
        return EXIT_OP


def main() -> int:
    return _run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
