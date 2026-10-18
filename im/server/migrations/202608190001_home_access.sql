-- Celira Core V1：Home 身份、Pairing Grant 与 Session Metadata（A 包窄解冻切片）。
--
-- 契约：CORE_V1_IM_CONTRACT.md §3.1（Home 身份、设备配对与 Session 管理）
-- - Home Identity：每个 Core 数据库恰一条，非秘密、不可变；初始化以
--   home_guard（id=1 唯一行）在同一事务内幂等保证「只成功一次」；
--   home_id 必须是规范小写连字符 UUID v4；display_name 可选（NULL=无标题）；
--   created_at 由应用生成，禁止客户端提交；行一经插入不可 UPDATE/DELETE。
-- - pairing_grants：一次性、短期、可撤销配对凭据。token 只存 SHA-256
--   十六进制摘要（64 字符、UNIQUE），原始 token 永不落盘；issued_by 必须
--   指向 active Human（应用层在签发事务内裁决，数据库以外键 + Human
--   方向触发器兜底）；device_label 可选非秘密标签；status 状态机：
--   active → consumed（一次性兑换成功，同一事务写 consumed_at）、
--   active → revoked（主人撤销，写 revoked_at）、或按 expires_at 派生
--   expired（TTL 内未用自动失效，不落库写状态）；consumed/revoked 是
--   终态，不得反向或互相转换；issued_at/expires_at/token_hash/issued_by/
--   device_label/grant_id 一经插入不可改写。
-- - session_metadata：新事实，1:1 引用既有 human_sessions（session_id
--   主键）；只存非秘密 device 标签与来源 grant；绝不包含 token/hash；
--   不改写 human_sessions 既有语义（撤销/列表仍以 human_sessions 为准，
--   metadata 只是可选附注）；行一经插入不可 UPDATE/DELETE。

-- 每 Core 恰一条 Home Identity 的 guard：与 bootstrap_guard 同模式，
-- guard 与 home 行必须在同一事务内提交。
CREATE TABLE home_guard (
    id INTEGER PRIMARY KEY CHECK (id = 1)
);

CREATE TABLE home (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    home_id      TEXT NOT NULL,
    display_name TEXT,
    created_at   INTEGER NOT NULL,
    CHECK (typeof(home_id) = 'text'),
    CHECK (length(home_id) = 36),
    CHECK (substr(home_id, 9, 1) = '-'),
    CHECK (substr(home_id, 14, 1) = '-'),
    CHECK (substr(home_id, 19, 1) = '-'),
    CHECK (substr(home_id, 24, 1) = '-'),
    CHECK (substr(home_id, 15, 1) = '4'),
    CHECK (substr(home_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (home_id = lower(home_id)),
    CHECK (instr(home_id, char(0)) = 0),
    CHECK (home_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(home_id, '-', '')) = 32),
    CHECK (display_name IS NULL OR length(display_name) <= 80),
    CHECK (display_name IS NULL OR instr(display_name, char(0)) = 0),
    CHECK (display_name IS NULL OR trim(display_name, char(9) || char(10) || char(13) || ' ') <> '')
);

-- Home 行不可变：禁止一切 UPDATE 与 DELETE（REPLACE 属于 DELETE+INSERT，
-- 同样被 DELETE 触发器拦截）。
CREATE TRIGGER home_row_immutable_update
BEFORE UPDATE ON home
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'home row is immutable');
END;

CREATE TRIGGER home_row_immutable_delete
BEFORE DELETE ON home
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'home row is immutable');
END;

-- 第二次 INSERT（含 INSERT OR REPLACE）必须在删除旧行之前失败，避免
-- conflict resolution 绕过 Home Identity 的不可变与单例约束。
CREATE TRIGGER home_row_singleton_insert
BEFORE INSERT ON home
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM home WHERE id = 1)
BEGIN
    SELECT RAISE(ABORT, 'home identity already exists');
END;

CREATE TABLE pairing_grants (
    grant_id     TEXT PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    issued_by    TEXT NOT NULL REFERENCES participants(id),
    device_label TEXT,
    status       TEXT NOT NULL CHECK (status IN ('active', 'consumed', 'revoked')),
    issued_at    INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    consumed_at  INTEGER,
    revoked_at   INTEGER,
    CHECK (typeof(grant_id) = 'text'),
    CHECK (length(grant_id) = 36),
    CHECK (substr(grant_id, 9, 1) = '-'),
    CHECK (substr(grant_id, 14, 1) = '-'),
    CHECK (substr(grant_id, 19, 1) = '-'),
    CHECK (substr(grant_id, 24, 1) = '-'),
    CHECK (substr(grant_id, 15, 1) = '4'),
    CHECK (substr(grant_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (grant_id = lower(grant_id)),
    CHECK (instr(grant_id, char(0)) = 0),
    CHECK (grant_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(grant_id, '-', '')) = 32),
    CHECK (expires_at > issued_at),
    CHECK (device_label IS NULL OR length(device_label) <= 80),
    CHECK (device_label IS NULL OR instr(device_label, char(0)) = 0),
    CHECK (device_label IS NULL OR trim(device_label, char(9) || char(10) || char(13) || ' ') <> ''),
    CHECK ((status = 'active' AND consumed_at IS NULL AND revoked_at IS NULL)
        OR (status = 'consumed' AND consumed_at IS NOT NULL AND revoked_at IS NULL)
        OR (status = 'revoked' AND revoked_at IS NOT NULL AND consumed_at IS NULL))
);

CREATE INDEX idx_pairing_grants_issued_by ON pairing_grants(issued_by);

-- 签发者方向：issued_by 必须指向 Human Participant（不是 Agent）。
CREATE TRIGGER pairing_grants_issued_by_human
BEFORE INSERT ON pairing_grants
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.issued_by AND kind = 'human')
BEGIN
    SELECT RAISE(ABORT, 'pairing grant issued_by must reference a human');
END;

-- 签发事实不可变：token_hash / issued_by / device_label / 时间戳一经插入
-- 不可改写；grant_id 是主键（UPDATE 主键等价 DELETE+INSERT，同样被拦截）。
CREATE TRIGGER pairing_grants_issue_fields_immutable
BEFORE UPDATE OF grant_id, token_hash, issued_by, device_label, issued_at, expires_at
ON pairing_grants
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'pairing grant issue fields are immutable');
END;

-- 状态机终态守卫：consumed / revoked 都是终态；`expired` 是应用层从
-- active + expires_at 派生的展示状态，不是可伪造的持久状态。active →
-- consumed 必须已经有同 Grant 的 Session Metadata（Exchange 在同一事务
-- 内先插 Metadata 再标记 consumed）；active → revoked 必须写 revoked_at。
CREATE TRIGGER pairing_grants_status_terminal
BEFORE UPDATE OF status, consumed_at, revoked_at ON pairing_grants
FOR EACH ROW
WHEN (OLD.status <> 'active'
      AND (NEW.status <> OLD.status OR NEW.consumed_at IS NOT OLD.consumed_at
           OR NEW.revoked_at IS NOT OLD.revoked_at))
  OR (OLD.status = 'active' AND NEW.status = 'consumed'
      AND (NEW.consumed_at IS NULL OR NEW.revoked_at IS NOT NULL
           OR NEW.consumed_at > (SELECT expires_at FROM pairing_grants WHERE grant_id = OLD.grant_id)
           OR NOT EXISTS (SELECT 1 FROM session_metadata WHERE source_grant_id = OLD.grant_id)))
  OR (OLD.status = 'active' AND NEW.status = 'revoked'
      AND (NEW.revoked_at IS NULL OR NEW.consumed_at IS NOT NULL))
  OR (OLD.status = 'active' AND NEW.status = 'active'
      AND (NEW.consumed_at IS NOT NULL OR NEW.revoked_at IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'pairing grant terminal status is immutable');
END;

-- Pairing Grant 是公共审计事实：无论 active/revoked/consumed 均不可删；
-- `REPLACE` 同样会在删除旧行时被拒绝。
CREATE TRIGGER pairing_grants_immutable_delete
BEFORE DELETE ON pairing_grants
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'pairing grant is immutable');
END;

-- session_metadata：1:1 引用 human_sessions（session_id 主键），
-- 只存非秘密附注；一经插入不可变。
CREATE TABLE session_metadata (
    session_id     TEXT PRIMARY KEY REFERENCES human_sessions(id),
    device_label   TEXT,
    source_grant_id TEXT NOT NULL REFERENCES pairing_grants(grant_id),
    UNIQUE (source_grant_id),
    CHECK (device_label IS NULL OR length(device_label) <= 80),
    CHECK (device_label IS NULL OR instr(device_label, char(0)) = 0),
    CHECK (device_label IS NULL OR trim(device_label, char(9) || char(10) || char(13) || ' ') <> '')
);

-- 来源 Grant 必须属于该 Session 的 Human。Exchange 会在同一事务内先创建
-- Session/Metadata、再将 Grant 标为 consumed；此守卫让直接 SQL 也不能
-- 伪造跨 Human 的审计来源关系。
CREATE TRIGGER session_metadata_source_grant_same_human
BEFORE INSERT ON session_metadata
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM human_sessions AS s
    JOIN pairing_grants AS g ON g.grant_id = NEW.source_grant_id
    WHERE s.id = NEW.session_id AND s.human_id = g.issued_by
)
BEGIN
    SELECT RAISE(ABORT, 'session metadata source grant must belong to session human');
END;

CREATE TRIGGER session_metadata_immutable_update
BEFORE UPDATE ON session_metadata
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'session metadata is immutable');
END;

CREATE TRIGGER session_metadata_immutable_delete
BEFORE DELETE ON session_metadata
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'session metadata is immutable');
END;
