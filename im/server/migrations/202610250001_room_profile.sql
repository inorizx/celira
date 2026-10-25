-- Celira Core v1.1 附加切片：Group Room Profile（群简介）。
--
-- 契约：docs/contracts/core-v1/CORE_V1_ROOM_PROFILE.md
--
-- 事实模型：
-- - 房间简介是 **group Room 自己的公共资料**：与 message / delivery / read
--   cursor / event 主链完全无关，不产生未读或通知。`rooms` 表一字不动
--   （房名仍走既有 Group Room Presentation），本迁移只新增简介的不可变版本链。
-- - room_profile_versions：(room_id, version) 主键，无行 = 没有简介；每行
--   version > 0、同 room 严格单调 +1（触发器）、一经插入不可 UPDATE/DELETE
--   （REPLACE 以 BEFORE INSERT identity guard 拒绝）。
-- - 只有 kind='group' 的 active Room 允许写；写入者必须是该 Room 的 active
--   owner 且是 active Human（应用层与 DB 触发器同一判据）。owner 转让后新
--   owner 自然可写——授权读的是当前 active owner 周期，不是任何身份缓存。
-- - room_profile_put_idempotency：照 room_presentation_put_idempotency 的
--   形状，(actor_human_id, room_id, key) 复合主键，只存 SHA-256 请求摘要
--   （64 位小写 hex）与结果版本，绝不复制简介正文。deferred 复合外键只证明
--   「引用一致性」（result_version 指向真实存在且同 room 的版本行），**不能**
--   单独证明两行在同一事务内插入；同事务原子性由 adapter 的单个
--   `BEGIN IMMEDIATE` 保证（见契约 §6）。
--
-- 字节长度：SQLite 的 length(text) 按字符计数，因此 2000 字节上限必须用
-- length(CAST(description AS BLOB)) 裁决；U+0000 用 instr(description,
-- char(0)) = 0 拒绝。应用层按 Rust `str::trim()` 判「trim 后非空」；SQL 层
-- 用同一集合（空格 / Tab / CR / LF）兜底，二者都不做文本归一化——存的是
-- 提交时的原文。
--
-- 时间戳由 Core 生成（updated_at / created_at），客户端不得指定。
-- 触发器全部为 BEFORE INSERT/UPDATE/DELETE 路由内的守卫，与既有迁移同一风格。

-- ============================================================
-- 1. Group Room Profile 版本链
-- ============================================================

CREATE TABLE room_profile_versions (
    room_id     TEXT NOT NULL REFERENCES rooms(id),
    version     INTEGER NOT NULL CHECK (version > 0),
    description TEXT NOT NULL,
    updated_by  TEXT NOT NULL REFERENCES participants(id),
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id, version),
    CHECK (typeof(description) = 'text'
        AND instr(description, char(0)) = 0
        AND length(CAST(description AS BLOB)) BETWEEN 1 AND 2000
        -- trim 后非空：只含空白（空格 / Tab / CR / LF）的正文不是简介。
        AND trim(description, char(9) || char(10) || char(13) || ' ') <> '')
);

CREATE TRIGGER room_profile_versions_immutable
BEFORE UPDATE ON room_profile_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room profile versions are immutable');
END;

CREATE TRIGGER room_profile_versions_no_delete
BEFORE DELETE ON room_profile_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room profile versions cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id, version) 冲突时先隐式删除冲突行，
-- 可绕过 immutable / no-delete 守卫改写或重建既有版本。
CREATE TRIGGER room_profile_versions_insert_identity_guard
BEFORE INSERT ON room_profile_versions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_profile_versions
    WHERE room_id = NEW.room_id AND version = NEW.version
)
BEGIN
    SELECT RAISE(ABORT, 'room profile version identity already exists');
END;

-- 版本严格单调：新版本必须等于该 room 当前最大版本 + 1（无行 => 1）。
-- 缺号 / 跳号 / 重复一律拒绝；base_version 的并发裁决依赖此不变量。
CREATE TRIGGER room_profile_versions_strictly_increment
BEFORE INSERT ON room_profile_versions
FOR EACH ROW
WHEN NEW.version <> (
    SELECT COALESCE(MAX(version), 0) + 1
    FROM room_profile_versions
    WHERE room_id = NEW.room_id
)
BEGIN
    SELECT RAISE(ABORT, 'room profile version must be exactly one more than the latest for this room');
END;

-- 写入资格（数据库兜底）：Room 必须是 active 的 group（direct / human_direct
-- 没有群简介）；写入者必须是 active Human 且是该 Room 当前 active owner。
-- owner 转让后旧 owner 的周期已关闭 ⇒ 同一守卫自动改判新 owner。
CREATE TRIGGER room_profile_versions_insert_guards
BEFORE INSERT ON room_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r
    WHERE r.id = NEW.room_id AND r.kind = 'group' AND r.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.updated_by AND p.kind = 'human' AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships o
    WHERE o.room_id = NEW.room_id
      AND o.participant_id = NEW.updated_by
      AND o.role = 'owner'
      AND o.active_marker = 1
)
BEGIN
    SELECT RAISE(ABORT, 'room profile requires an active group room and its active human owner');
END;

-- ============================================================
-- 2. Group Room Profile 幂等记录
-- ============================================================

CREATE TABLE room_profile_put_idempotency (
    actor_human_id TEXT NOT NULL REFERENCES participants(id),
    room_id        TEXT NOT NULL REFERENCES rooms(id),
    key            TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    result_version INTEGER NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (actor_human_id, room_id, key),
    -- deferred 复合外键：result_version 引用 room_profile_versions 主键。
    -- 它只证明**引用一致性**（记录的版本字段指向真实存在且同 room 的版本
    -- 行），不能单独证明两行在同一事务内插入；同事务原子性由 adapter 的
    -- 单个 `BEGIN IMMEDIATE` 保证（见契约 §6）。
    FOREIGN KEY (room_id, result_version)
        REFERENCES room_profile_versions(room_id, version)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (length(request_digest) = 64),
    CHECK (request_digest = lower(request_digest)),
    CHECK (instr(request_digest, char(0)) = 0),
    CHECK (request_digest NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录只可能由 active Human 创建（actor 即写入者）。
CREATE TRIGGER room_profile_put_idempotency_actor_human
BEFORE INSERT ON room_profile_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency actor must be an active human');
END;

CREATE TRIGGER room_profile_put_idempotency_immutable
BEFORE UPDATE ON room_profile_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency records are immutable');
END;

CREATE TRIGGER room_profile_put_idempotency_no_delete
BEFORE DELETE ON room_profile_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键冲突时先隐式删除冲突行，可绕过 no-delete /
-- immutable 守卫重映射 key 到另一请求 / 版本。
CREATE TRIGGER room_profile_put_idempotency_insert_identity_guard
BEFORE INSERT ON room_profile_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_profile_put_idempotency
    WHERE actor_human_id = NEW.actor_human_id
      AND room_id = NEW.room_id
      AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency identity already exists');
END;

-- 见证一致性：记录 actor 必须等于被引用版本行的写入者，created_at 必须等于
-- 该版本的 updated_at（同一 Core 操作时间）。adapter 在同一事务内先插版本行
-- 再插本记录，保证本守卫永远可见被引用行；本守卫只证明身份 / 引用一致性，
-- 不证明同事务（deferred FK 亦不证明），见契约 §6。
CREATE TRIGGER room_profile_put_idempotency_witness_consistency
BEFORE INSERT ON room_profile_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_profile_versions v
    WHERE v.room_id = NEW.room_id
      AND v.version = NEW.result_version
      AND (v.updated_by <> NEW.actor_human_id OR v.updated_at <> NEW.created_at)
)
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency witness must match the result version writer and time');
END;
