#!/usr/bin/env python3
"""home_portability.py 的 stdlib unittest：构造临时真实 SQLite + attachments，
覆盖 backup→verify→restore 成功路径与篡改/路径穿越/symlink/权限/边界失败路径。

只依赖 Python 3 标准库（unittest，不新增依赖）。运行：
    python3 -m unittest discover -s tools/tests -p 'test_*.py' -v
"""

import hashlib
import importlib.util
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import unittest
import uuid as uuid_mod

TOOL = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "home_portability.py"))
UUID4 = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")

HOME_ID = "11111111-2222-4aaa-8bbb-ccddeeff0011"


def run_cli(*argv):
    return subprocess.run(
        [sys.executable, TOOL, *argv],
        capture_output=True,
        text=True,
    )


def load_tool_module():
    spec = importlib.util.spec_from_file_location("home_portability", TOOL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class FakeHome:
    """真实 SQLite（自建 home/_sqlx_migrations 等最小归属事实）+ attachments 目录。"""

    def __init__(self, base: str):
        self.base = base
        self.db = os.path.join(base, "core.db")
        self.media = os.path.join(base, "media")
        self.attachments = os.path.join(self.media, "attachments")
        os.makedirs(self.attachments, exist_ok=True)
        conn = sqlite3.connect(self.db)
        conn.executescript(
            """
            CREATE TABLE _sqlx_migrations (
              version BIGINT PRIMARY KEY, description TEXT NOT NULL,
              installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
              success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);
            INSERT INTO _sqlx_migrations
              VALUES (202608110001,'core_baseline',CURRENT_TIMESTAMP,1,X'00',0);
            INSERT INTO _sqlx_migrations
              VALUES (202608190001,'home_access',CURRENT_TIMESTAMP,1,X'01',0);
            CREATE TABLE home_guard (id INTEGER PRIMARY KEY CHECK (id=1));
            INSERT INTO home_guard VALUES (1);
            CREATE TABLE home (
              id INTEGER PRIMARY KEY CHECK (id=1),
              home_id TEXT NOT NULL, display_name TEXT, created_at INTEGER NOT NULL);
            INSERT INTO home VALUES (1, '""" + HOME_ID + r"""','Test Home',0);
            """
        )
        conn.commit()
        conn.close()

    def add_attachment(self, data: bytes, name: str | None = None) -> str:
        name = name or str(uuid_mod.uuid4())
        with open(os.path.join(self.attachments, name), "wb") as f:
            f.write(data)
        return name


class HomePortabilityToolTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.base = os.path.join(self.tmp.name, "home")
        os.makedirs(self.base)
        self.home = FakeHome(self.base)

    def tearDown(self):
        self.tmp.cleanup()

    def _backup(self, output, **extra):
        args = [
            "backup",
            "--db", self.home.db,
            "--media-root", self.home.media,
            "--output", output,
        ]
        if extra.get("confirm", True):
            args.append("--confirm-core-stopped")
        return run_cli(*args)

    # ---------------- backup 失败路径 ----------------

    def test_backup_requires_explicit_confirm(self):
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out, confirm=False)
        self.assertEqual(r.returncode, 2)
        self.assertNotIn("token", r.stdout)

    def test_backup_fails_when_output_exists(self):
        out = os.path.join(self.tmp.name, "bk")
        os.makedirs(out)
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("output_exists", r.stderr)

    def test_backup_fails_when_db_missing(self):
        out = os.path.join(self.tmp.name, "bk")
        r = run_cli("backup", "--db", os.path.join(self.base, "nope.db"),
                    "--media-root", self.home.media, "--output", out,
                    "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertIn("database_missing", r.stderr)

    def test_backup_fails_when_db_not_regular(self):
        out = os.path.join(self.tmp.name, "bk")
        d = os.path.join(self.base, "a_dir")
        os.makedirs(d)
        r = run_cli("backup", "--db", d, "--media-root", self.home.media,
                    "--output", out, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertIn("database_not_regular", r.stderr)

    def test_backup_fails_on_wal_sidecar(self):
        open(self.home.db + "-wal", "w").write("junk")
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("db_unclean_stop", r.stderr)
        self.assertFalse(os.path.exists(out))

    def test_backup_fails_on_rollback_journal(self):
        """**本构建真正会留下的那个**：rollback journal（`-journal`），不是 WAL。

        为什么单列这一条：`im/server/src` 里没有任何 `journal_mode` pragma ⇒ SQLite 走
        默认的 DELETE 模式 ⇒ 硬停留下 `core.db-journal`。从前只查 `-wal`/`-shm`，
        这道 fail-closed 在真机上**永不触发**（2026-09-25 发现）。
        """
        open(self.home.db + "-journal", "w").write("junk")
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("db_unclean_stop", r.stderr)
        self.assertIn("-journal", r.stderr, "读数要点名是哪一个兄弟文件")
        self.assertFalse(os.path.exists(out))

    def test_backup_fails_on_shm_sidecar(self):
        """WAL 模式的第二个兄弟（`-shm`）同样算"没干净停机"（三种后缀都查）。"""
        open(self.home.db + "-shm", "w").write("junk")
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("db_unclean_stop", r.stderr)
        self.assertFalse(os.path.exists(out))

    def test_backup_rejects_unknown_attachment_name(self):
        with open(os.path.join(self.home.attachments, "not-a-uuid.txt"), "w") as f:
            f.write("x")
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("attachment_unknown_name", r.stderr)
        self.assertFalse(os.path.exists(out))

    def test_backup_rejects_subdirectory_in_attachments(self):
        sub = os.path.join(self.home.attachments, "subdir")
        os.makedirs(sub)
        with open(os.path.join(sub, "file.bin"), "w") as f:
            f.write("x")
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("attachment_unknown_name", r.stderr)

    def test_backup_rejects_symlink_in_attachments(self):
        target = os.path.join(self.base, "outside.bin")
        with open(target, "wb") as f:
            f.write(b"secret-outside")
        os.symlink(target, os.path.join(self.home.attachments, str(uuid_mod.uuid4())))
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("attachment_symlink", r.stderr)

    def test_backup_fails_when_home_table_missing(self):
        conn = sqlite3.connect(self.home.db)
        conn.execute("DROP TABLE home")
        conn.commit()
        conn.close()
        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("db_home_read_failed", r.stderr)

    def test_backup_fails_when_output_inside_media_root(self):
        out = os.path.join(self.home.media, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("output_inside_source", r.stderr)

    # ---------------- 成功 roundtrip ----------------

    def test_backup_verify_restore_roundtrip(self):
        a1 = self.home.add_attachment(b"hello attachment bytes")
        a2 = self.home.add_attachment(b"second\x00with\x00nuls")
        tmp_name = f"{uuid_mod.uuid4()}.tmp"
        self.home.add_attachment(b"residual", name=tmp_name)

        out = os.path.join(self.tmp.name, "bk")
        r = self._backup(out)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("token", r.stdout)
        report = json.loads(r.stdout)
        self.assertEqual(report["home_id"], HOME_ID)
        self.assertEqual(report["migration_count"], 2)
        self.assertEqual(report["attachment_count"], 3)

        # 权限：目录 0700 / 文件 0600。
        mode = lambda p: os.stat(p).st_mode & 0o777
        self.assertEqual(mode(out), 0o700)
        self.assertEqual(mode(os.path.join(out, "core.db")), 0o600)
        self.assertEqual(mode(os.path.join(out, "media")), 0o700)
        self.assertEqual(mode(os.path.join(out, "media", "attachments")), 0o700)
        for name in (a1, a2, tmp_name):
            self.assertEqual(mode(os.path.join(out, "media", "attachments", name)), 0o600)

        manifest = json.load(open(os.path.join(out, "manifest.json")))
        self.assertEqual(manifest["format_version"], 1)
        self.assertEqual(manifest["home_id"], HOME_ID)
        self.assertTrue(all(m["success"] is True for m in manifest["applied_migrations"]))
        for item in manifest["applied_migrations"]:
            self.assertNotIn("checksum", item)
        self.assertEqual(manifest["attachment_count"], 3)

        # verify 不修改备份。
        hashes_before = {}
        for f in os.listdir(out):
            p = os.path.join(out, f)
            if os.path.isfile(p):
                hashes_before[f] = hashlib.sha256(open(p, "rb").read()).hexdigest()
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 0, r.stderr)
        for f, h in hashes_before.items():
            p = os.path.join(out, f)
            self.assertEqual(hashlib.sha256(open(p, "rb").read()).hexdigest(), h)
        for root, _, files in os.walk(os.path.join(out, "media")):
            for f in files:
                self.assertNotIn(f, hashes_before)

        # restore 到不存在目标。
        dest = os.path.join(self.tmp.name, "rest")
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(mode(dest), 0o700)
        for name in (a1, a2, tmp_name):
            self.assertEqual(
                open(os.path.join(dest, "media", "attachments", name), "rb").read(),
                open(os.path.join(self.home.attachments, name), "rb").read(),
            )
        self.assertTrue(os.path.isfile(os.path.join(dest, "restore-manifest.json")))
        marker = json.load(open(os.path.join(dest, "restore-manifest.json")))
        self.assertEqual(marker["source_backup"]["home_id"], HOME_ID)
        # 恢复出的 DB 可只读打开且 home 一致。
        conn = sqlite3.connect(f"file:{os.path.join(dest, 'core.db')}?mode=ro", uri=True)
        home_id = conn.execute("SELECT home_id FROM home WHERE id=1").fetchone()[0]
        conn.close()
        self.assertEqual(home_id, HOME_ID)

    def test_restore_into_empty_existing_directory(self):
        out = os.path.join(self.tmp.name, "bk")
        self.assertEqual(self._backup(out).returncode, 0)
        dest = os.path.join(self.tmp.name, "rest_empty")
        os.makedirs(dest)
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(os.path.isfile(os.path.join(dest, "core.db")))

    # ---------------- verify 失败路径 ----------------

    def _make_valid_backup(self):
        self.home.add_attachment(b"payload-1")
        out = os.path.join(self.tmp.name, "bk")
        self.assertEqual(self._backup(out).returncode, 0)
        return out

    def test_verify_fails_manifest_tampered_hash(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["core.db"]["sha256"] = "0" * 64
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_db_hash_mismatch", r.stderr)

    def test_verify_fails_db_tampered(self):
        out = self._make_valid_backup()
        with open(os.path.join(out, "core.db"), "r+b") as f:
            f.seek(100)
            b = f.read(1)
            f.seek(100)
            f.write(bytes([b[0] ^ 0xFF]))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_", r.stderr)

    def test_verify_fails_attachment_missing(self):
        out = self._make_valid_backup()
        attdir = os.path.join(out, "media", "attachments")
        os.unlink(os.path.join(attdir, os.listdir(attdir)[0]))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_missing_files", r.stderr)

    def test_verify_fails_attachment_tampered(self):
        out = self._make_valid_backup()
        attdir = os.path.join(out, "media", "attachments")
        p = os.path.join(attdir, os.listdir(attdir)[0])
        with open(p, "r+b") as f:
            b = f.read(1)
            f.seek(0)
            f.write(bytes([b[0] ^ 0xFF]))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_attachment_hash_mismatch", r.stderr)

    def test_verify_fails_path_traversal_in_manifest(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["attachments"][0]["relative_path"] = "../escape/a"
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_unsafe_path", r.stderr)

    def test_verify_fails_absolute_path_in_manifest(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["attachments"][0]["relative_path"] = "/etc/passwd"
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_absolute_path", r.stderr)

    def test_verify_fails_unknown_manifest_format(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["format_version"] = 99
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_unsupported_version", r.stderr)

    def test_verify_fails_extra_unlisted_file(self):
        out = self._make_valid_backup()
        with open(os.path.join(out, "media", "attachments", str(uuid_mod.uuid4())), "w") as f:
            f.write("stray")
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_unexpected_files", r.stderr)

    def test_verify_fails_symlink_inside_backup(self):
        out = self._make_valid_backup()
        os.symlink(os.path.join(out, "core.db"),
                   os.path.join(out, "media", "attachments", str(uuid_mod.uuid4())))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_symlink", r.stderr)

    def test_verify_fails_when_backup_dir_missing(self):
        r = run_cli("verify", "--backup", os.path.join(self.tmp.name, "missing"))
        self.assertEqual(r.returncode, 1)

    def test_verify_fails_db_with_failed_migration(self):
        out = self._make_valid_backup()
        conn = sqlite3.connect(f"file:{os.path.join(out,'core.db')}?mode=rw", uri=True)
        conn.execute("UPDATE _sqlx_migrations SET success=0 WHERE version=202608110001")
        conn.commit()
        conn.close()
        # 改库后 manifest 的 core.db sha256 不再匹配，需同步 manifest 才能命中迁移失败。
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        data = open(os.path.join(out, "core.db"), "rb").read()
        m["core.db"]["sha256"] = hashlib.sha256(data).hexdigest()
        m["core.db"]["size"] = len(data)
        m["applied_migrations"][0]["success"] = False
        json.dump(m, open(mp, "w"))
        for item in m["applied_migrations"]:
            if not item["success"]:
                break
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_migration_failed", r.stderr)

    # ---------------- 结构/清单 严格校验新增用例 ----------------

    def test_verify_fails_symlink_directory_in_backup_tree(self):
        out = self._make_valid_backup()
        target = os.path.join(self.tmp.name, "elsewhere")
        os.makedirs(target)
        os.symlink(target, os.path.join(out, "media", "evil"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_symlink", r.stderr)

    def test_verify_fails_unknown_empty_dir_under_media(self):
        out = self._make_valid_backup()
        os.makedirs(os.path.join(out, "media", "unknown"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_unexpected_dirs", r.stderr)

    def test_verify_fails_unknown_empty_dir_in_attachments(self):
        out = self._make_valid_backup()
        os.makedirs(os.path.join(out, "media", "attachments", "sub"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_unexpected_dirs", r.stderr)

    def test_verify_fails_missing_attachments_dir_even_when_empty(self):
        out = os.path.join(self.tmp.name, "bk_empty")
        self.assertEqual(self._backup(out).returncode, 0)  # 空 attachments 也能成功备份
        os.rmdir(os.path.join(out, "media", "attachments"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_missing_files", r.stderr)

    def test_verify_fails_manifest_count_mismatch(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["attachment_count"] = m["attachment_count"] + 1
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_count_mismatch", r.stderr)

    def test_verify_fails_manifest_bytes_mismatch(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["attachment_bytes"] = m["attachment_bytes"] + 1
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_bytes_mismatch", r.stderr)

    def test_verify_fails_wrong_tool_in_manifest(self):
        out = self._make_valid_backup()
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["tool"] = "evil/tool.py"
        json.dump(m, open(mp, "w"))
        r = run_cli("verify", "--backup", out)
        self.assertEqual(r.returncode, 1)
        self.assertIn("manifest_tool_mismatch", r.stderr)

    def test_backup_copy_mismatch_fails_and_cleans_output(self):
        hp = load_tool_module()
        copied = []

        def broken_copy(src, dst, chunk=1 << 16):
            copied.append(dst)
            return 0, "0" * 64

        hp._copy_stream_hashed = broken_copy
        out = os.path.join(self.tmp.name, "bk_mismatch")
        code = hp._run([
            "backup", "--db", self.home.db, "--media-root", self.home.media,
            "--output", out, "--confirm-core-stopped",
        ])
        self.assertNotEqual(code, 0)
        self.assertFalse(os.path.exists(out))
        self.assertGreaterEqual(len(copied), 1)

    # ---------------- restore 失败路径 ----------------

    def test_restore_requires_confirm(self):
        out = self._make_valid_backup()
        dest = os.path.join(self.tmp.name, "rest")
        r = run_cli("restore", "--backup", out, "--destination", dest)
        self.assertEqual(r.returncode, 2)
        self.assertFalse(os.path.exists(dest))

    def test_restore_fails_when_destination_nonempty(self):
        out = self._make_valid_backup()
        dest = os.path.join(self.tmp.name, "rest")
        os.makedirs(os.path.join(dest, "existing"))
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertIn("destination_not_empty", r.stderr)
        self.assertEqual(os.listdir(dest), ["existing"])

    def test_restore_fails_when_destination_is_file(self):
        out = self._make_valid_backup()
        dest = os.path.join(self.tmp.name, "rest_file")
        open(dest, "w").write("occupied")
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertIn("destination_exists_file", r.stderr)
        self.assertEqual(open(dest).read(), "occupied")

    def test_restore_fails_when_backup_invalid(self):
        out = self._make_valid_backup()
        os.unlink(os.path.join(out, "core.db"))
        dest = os.path.join(self.tmp.name, "rest")
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertIn("backup_missing_files", r.stderr)
        self.assertFalse(os.path.exists(dest))

    def test_restore_leaves_no_temp_sibling_on_failure(self):
        out = self._make_valid_backup()
        # 破坏 manifest 以在 restore 的 verify 阶段失败。
        mp = os.path.join(out, "manifest.json")
        m = json.load(open(mp))
        m["attachments"][0]["relative_path"] = "../escape/a"
        json.dump(m, open(mp, "w"))
        dest = os.path.join(self.tmp.name, "rest")
        r = run_cli("restore", "--backup", out, "--destination", dest, "--confirm-core-stopped")
        self.assertEqual(r.returncode, 1)
        self.assertFalse(os.path.exists(dest))
        leftovers = [d for d in os.listdir(self.tmp.name) if d.startswith(".home-restore-")]
        self.assertEqual(leftovers, [])


if __name__ == "__main__":
    unittest.main()
