-- Celira Core V1：Participant Presentation 切片（Participant Public Profile + Group Room Presentation）。
--
-- 契约：CORE_V1_PARTICIPANT_PRESENTATION.md
-- 本迁移只新增本切片公共事实，不改动任何既有冻结语义（messages/memberships/
-- events/completions/acks/permissions 等一律不动）。
--
-- 事实模型：
-- - participant_profile_versions：参与者（P0 只写 Agent，模型按 Participant
--   建模）全局 Profile 的不可变版本链。(participant_id, profile_version) 主键，
--   无行 = 版本 0（null 空投影）；每行版本 > 0；同 participant 严格单调 +1
--   （触发器），一经插入不可 UPDATE/DELETE（REPLACE 以 BEFORE INSERT identity
--   guard 拒绝）；不复制 display_name/status/kind。
-- - room_presentation_versions：Group Room 内某个 active Agent 成员的呈现版本链。
--   (room_id, participant_id, presentation_version) 主键；只允许 kind='group'、
--   active Agent member + active group owner（Human）发布；单调 +1 / immutable /
--   no-delete / REPLACE guard 同 Profile。
-- - participant_profile_put_idempotency / room_presentation_put_idempotency：
--   (actor_human_id, participant_id[, room_id], key) 复合主键幂等记录，只存
--   SHA-256 请求摘要（64 位小写 hex）与结果版本，绝不复制描述/正文/预览。
--   deferred 复合外键只证明「引用一致性」（result_*_version 指向真实存在且
--   身份一致的版本行），**不能**单独证明两行在同一事务内插入；同事务原子性
--   由后续 adapter 的单个 `BEGIN IMMEDIATE` 事务保证（本迁移的触发器与 CHECK
--   仅证明版本行身份：result_is_create == (result_version = 1)、版本行的
--   published_by_id == 幂等 actor、published_at == created_at）。
--
-- 字节长度：SQLite 的 length(text) 按字符数计数，因此所有字节上限必须用
-- length(CAST(value AS BLOB)) 裁决；U+0000 用 instr(value, char(0)) = 0 拒绝。
-- 空串由应用规范化为 NULL；SQL 层**同样拒绝 ''**（不能成为与 NULL 并列的
-- 第二个持久事实），只保留空白组成的非空字符串（不 trim）为合法值。
--
-- 时间戳由 Core 生成（published_at/created_at），客户端不得指定。
-- 触发器全部为 BEFORE INSERT/UPDATE/DELETE 路由内的守卫，与既有迁移同一风格。

-- ============================================================
-- 1. Participant Profile 版本链
-- ============================================================

CREATE TABLE participant_profile_versions (
    participant_id     TEXT NOT NULL REFERENCES participants(id),
    profile_version    INTEGER NOT NULL CHECK (profile_version > 0),
    short_description  TEXT,
    public_description TEXT,
    published_by_id    TEXT NOT NULL REFERENCES participants(id),
    published_at       INTEGER NOT NULL,
    PRIMARY KEY (participant_id, profile_version),
    CHECK (short_description IS NULL
        OR (typeof(short_description) = 'text'
            AND instr(short_description, char(0)) = 0
            AND short_description <> ''
            AND length(CAST(short_description AS BLOB)) <= 512)),
    CHECK (public_description IS NULL
        OR (typeof(public_description) = 'text'
            AND instr(public_description, char(0)) = 0
            AND public_description <> ''
            AND length(CAST(public_description AS BLOB)) <= 16384))
);

CREATE TRIGGER participant_profile_versions_immutable
BEFORE UPDATE ON participant_profile_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'participant profile versions are immutable');
END;

CREATE TRIGGER participant_profile_versions_no_delete
BEFORE DELETE ON participant_profile_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'participant profile versions cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (participant_id, profile_version) 冲突时先隐式
-- 删除冲突行，可绕过 immutable/no-delete 守卫改写或重建既有版本。
CREATE TRIGGER participant_profile_versions_insert_identity_guard
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participant_profile_versions
    WHERE participant_id = NEW.participant_id
      AND profile_version = NEW.profile_version
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile version identity already exists');
END;

-- 版本严格单调：新版本必须等于该 participant 当前最大版本 + 1（无行 => 1）。
-- 缺号/跳号/重复一律拒绝；base_version 的并发裁决依赖此不变量。
CREATE TRIGGER participant_profile_version_strictly_increments
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NEW.profile_version <> (
    SELECT COALESCE(MAX(profile_version), 0) + 1
    FROM participant_profile_versions
    WHERE participant_id = NEW.participant_id
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile version must be exactly one more than the latest');
END;

-- 发布者必须是 active Human；目标必须是 active Agent，且发布者必须对该 Agent
-- 存在 active Owner 关系（P0 写路径；模型仍按 Participant 建模，这里以数据库
-- 兜底本切片的授权方向）。
CREATE TRIGGER participant_profile_versions_published_by_human
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.published_by_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile publisher must be an active human');
END;

CREATE TRIGGER participant_profile_versions_target_owned_agent
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM participants p
    JOIN agent_owners ao
      ON ao.agent_id = p.id AND ao.human_id = NEW.published_by_id AND ao.status = 'active'
    WHERE p.id = NEW.participant_id AND p.kind = 'agent' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile requires an active agent owned by the publisher');
END;

CREATE INDEX idx_profile_versions_published_by
    ON participant_profile_versions(published_by_id);

-- ============================================================
-- 2. Group Room Presentation 版本链
-- ============================================================

CREATE TABLE room_presentation_versions (
    room_id              TEXT NOT NULL REFERENCES rooms(id),
    participant_id       TEXT NOT NULL REFERENCES participants(id),
    presentation_version INTEGER NOT NULL CHECK (presentation_version > 0),
    room_display_name    TEXT,
    room_role_name       TEXT,
    room_role_description TEXT,
    published_by_id      TEXT NOT NULL REFERENCES participants(id),
    published_at         INTEGER NOT NULL,
    PRIMARY KEY (room_id, participant_id, presentation_version),
    CHECK (room_display_name IS NULL
        OR (typeof(room_display_name) = 'text'
            AND instr(room_display_name, char(0)) = 0
            AND room_display_name <> ''
            AND length(CAST(room_display_name AS BLOB)) <= 80)),
    CHECK (room_role_name IS NULL
        OR (typeof(room_role_name) = 'text'
            AND instr(room_role_name, char(0)) = 0
            AND room_role_name <> ''
            AND length(CAST(room_role_name AS BLOB)) <= 128)),
    CHECK (room_role_description IS NULL
        OR (typeof(room_role_description) = 'text'
            AND instr(room_role_description, char(0)) = 0
            AND room_role_description <> ''
            AND length(CAST(room_role_description AS BLOB)) <= 4096))
);

CREATE TRIGGER room_presentation_versions_immutable
BEFORE UPDATE ON room_presentation_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room presentation versions are immutable');
END;

CREATE TRIGGER room_presentation_versions_no_delete
BEFORE DELETE ON room_presentation_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room presentation versions cannot be deleted');
END;

-- 身份防改写（同前 REPLACE 防线）。
CREATE TRIGGER room_presentation_versions_insert_identity_guard
BEFORE INSERT ON room_presentation_versions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_presentation_versions
    WHERE room_id = NEW.room_id
      AND participant_id = NEW.participant_id
      AND presentation_version = NEW.presentation_version
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation version identity already exists');
END;

-- 版本严格单调：新版本 = 同 (room_id, participant_id) 当前最大版本 + 1。
CREATE TRIGGER room_presentation_version_strictly_increments
BEFORE INSERT ON room_presentation_versions
FOR EACH ROW
WHEN NEW.presentation_version <> (
    SELECT COALESCE(MAX(presentation_version), 0) + 1
    FROM room_presentation_versions
    WHERE room_id = NEW.room_id AND participant_id = NEW.participant_id
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation version must be exactly one more than the latest for this room/participant');
END;

-- 写入资格（数据库兜底）：Room 必须是**active** group（`rooms.status='active'`
-- 且 `kind='group'`；v1 schema 只允许 active，该状态条件是防御性纵深）；目标
-- 必须是该 Room 的 active Agent member（role='member' AND active_marker=1）；
-- 发布者必须是该 Room 的 active owner 且为 active Human。仅 Agent Owner 而该
-- Human 不是该 Room 的 active owner 一律拒绝（Owner 不产生 Room 权威）。
CREATE TRIGGER room_presentation_versions_insert_guards
BEFORE INSERT ON room_presentation_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r
    WHERE r.id = NEW.room_id AND r.kind = 'group' AND r.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.participant_id AND p.kind = 'agent' AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.room_id = NEW.room_id
      AND m.participant_id = NEW.participant_id
      AND m.role = 'member'
      AND m.active_marker = 1
)
OR NOT EXISTS (
    SELECT 1
    FROM room_memberships o
    JOIN participants p ON p.id = o.participant_id
    WHERE o.room_id = NEW.room_id
      AND o.role = 'owner'
      AND o.active_marker = 1
      AND p.id = NEW.published_by_id
      AND p.kind = 'human'
      AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation requires a group room, an active agent member, and the active human group owner as publisher');
END;

CREATE INDEX idx_room_presentation_versions_room_participant
    ON room_presentation_versions(room_id, participant_id);

-- ============================================================
-- 3. Agent Profile 幂等记录
-- ============================================================

CREATE TABLE participant_profile_put_idempotency (
    actor_human_id       TEXT NOT NULL REFERENCES participants(id),
    participant_id       TEXT NOT NULL REFERENCES participants(id),
    key                  TEXT NOT NULL,
    request_digest       TEXT NOT NULL CHECK (length(request_digest) = 64),
    result_profile_version INTEGER NOT NULL,
    result_is_create     INTEGER NOT NULL CHECK (result_is_create IN (0, 1)),
    created_at           INTEGER NOT NULL,
    PRIMARY KEY (actor_human_id, participant_id, key),
    -- deferred 复合外键：result_profile_version 引用 participant_profile_versions
    -- 主键。它只证明**引用一致性**（记录的版本字段指向真实存在的版本行），
    -- 不能单独证明两行在同一事务内插入；同事务原子性由 adapter 的单个
    -- `BEGIN IMMEDIATE` 保证（见本文件头部注释与 CORE_V1_PARTICIPANT_PRESENTATION.md §6）。
    FOREIGN KEY (participant_id, result_profile_version)
        REFERENCES participant_profile_versions(participant_id, profile_version)
        DEFERRABLE INITIALLY DEFERRED,
    -- identity 见证：result_is_create 必须恰好反映 result_profile_version == 1。
    CHECK (result_is_create = (result_profile_version = 1)),
    CHECK (request_digest = lower(request_digest)),
    CHECK (instr(request_digest, char(0)) = 0),
    CHECK (request_digest NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录只可能由 active Human 创建（actor 即发布者）。
CREATE TRIGGER participant_profile_put_idempotency_actor_human
BEFORE INSERT ON participant_profile_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'profile idempotency actor must be an active human');
END;

CREATE TRIGGER participant_profile_put_idempotency_immutable
BEFORE UPDATE ON participant_profile_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'participant profile idempotency records are immutable');
END;

CREATE TRIGGER participant_profile_put_idempotency_no_delete
BEFORE DELETE ON participant_profile_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'participant profile idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键冲突时先隐式删除冲突行，可绕过 no-delete /
-- immutable 守卫重映射 key 到另一请求/版本。
CREATE TRIGGER participant_profile_put_idempotency_insert_identity_guard
BEFORE INSERT ON participant_profile_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participant_profile_put_idempotency
    WHERE actor_human_id = NEW.actor_human_id
      AND participant_id = NEW.participant_id
      AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile idempotency identity already exists');
END;

-- 见证一致性：本记录的 actor 必须与被引用版本行的发布者一致
-- （version.published_by_id == actor_human_id），created_at 必须等于被引用
-- 版本的 published_at（同一 Core 操作时间）。adapter 在同一事务内先插版本行
-- 再插本记录，保证本守卫永远可见被引用行；本守卫只证明身份/引用一致性，
-- 不证明同事务（deferred FK 亦不证明），见 CORE_V1_PARTICIPANT_PRESENTATION.md §6。
CREATE TRIGGER participant_profile_put_idempotency_witness_consistency
BEFORE INSERT ON participant_profile_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participant_profile_versions v
    WHERE v.participant_id = NEW.participant_id
      AND v.profile_version = NEW.result_profile_version
      AND (v.published_by_id <> NEW.actor_human_id OR v.published_at <> NEW.created_at)
)
BEGIN
    SELECT RAISE(ABORT, 'profile idempotency witness must match the result version publisher and time');
END;

-- ============================================================
-- 4. Room Presentation 幂等记录
-- ============================================================

CREATE TABLE room_presentation_put_idempotency (
    actor_human_id         TEXT NOT NULL REFERENCES participants(id),
    room_id                TEXT NOT NULL REFERENCES rooms(id),
    participant_id         TEXT NOT NULL REFERENCES participants(id),
    key                    TEXT NOT NULL,
    request_digest         TEXT NOT NULL CHECK (length(request_digest) = 64),
    result_presentation_version INTEGER NOT NULL,
    result_is_create       INTEGER NOT NULL CHECK (result_is_create IN (0, 1)),
    created_at             INTEGER NOT NULL,
    PRIMARY KEY (actor_human_id, room_id, participant_id, key),
    -- deferred 复合外键：result_presentation_version 引用
    -- room_presentation_versions 主键。只证明**引用一致性**，不证明同事务
    -- 原子插入；同事务原子性由 adapter 的单个 `BEGIN IMMEDIATE` 保证。
    FOREIGN KEY (room_id, participant_id, result_presentation_version)
        REFERENCES room_presentation_versions(room_id, participant_id, presentation_version)
        DEFERRABLE INITIALLY DEFERRED,
    -- identity 见证：result_is_create 必须恰好反映 result_presentation_version == 1。
    CHECK (result_is_create = (result_presentation_version = 1)),
    CHECK (request_digest = lower(request_digest)),
    CHECK (instr(request_digest, char(0)) = 0),
    CHECK (request_digest NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER room_presentation_put_idempotency_actor_human
BEFORE INSERT ON room_presentation_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency actor must be an active human');
END;

CREATE TRIGGER room_presentation_put_idempotency_immutable
BEFORE UPDATE ON room_presentation_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency records are immutable');
END;

CREATE TRIGGER room_presentation_put_idempotency_no_delete
BEFORE DELETE ON room_presentation_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency records cannot be deleted');
END;

-- 身份防改写（同前 REPLACE 防线）。
CREATE TRIGGER room_presentation_put_idempotency_insert_identity_guard
BEFORE INSERT ON room_presentation_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_presentation_put_idempotency
    WHERE actor_human_id = NEW.actor_human_id
      AND room_id = NEW.room_id
      AND participant_id = NEW.participant_id
      AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency identity already exists');
END;

-- 见证一致性（同 Profile 记录）：actor 必须等于被引用版本的发布者，
-- created_at 必须等于被引用版本的 published_at（同一 Core 操作时间）。
-- 只证明身份/引用一致性，不证明同事务原子插入。
CREATE TRIGGER room_presentation_put_idempotency_witness_consistency
BEFORE INSERT ON room_presentation_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_presentation_versions v
    WHERE v.room_id = NEW.room_id
      AND v.participant_id = NEW.participant_id
      AND v.presentation_version = NEW.result_presentation_version
      AND (v.published_by_id <> NEW.actor_human_id OR v.published_at <> NEW.created_at)
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency witness must match the result version publisher and time');
END;
