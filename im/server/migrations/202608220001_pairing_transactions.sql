-- Celira Core V1：Pairing Transaction（A2 配对事务可靠提交协议，窄解冻切片）。
--
-- 契约：HOME_ACCESS.md §4.3（A2）+ §4.4（解冻影响标注）。本切片只新增
-- `pairing_transactions` 事实与 prepare/status/commit/abort 端点，不改写
-- 既有 women、Human Session、Grant、Membership、Message 语义：
--
-- - 事务通过 grant_id + client_nonce + session_token_hash + commit_token_hash
--   唯一识别（UNIQUE 四元组）；同一 nonce + 摘要重试恒返回同一准备事务，
--   参数逐字段不一致（新 nonce）则新建事务——两个标志各自独立裁决重放。
-- - session_token_hash / commit_token_hash 只存 SHA-256 十六进制摘要
--   （64 字符、各自 UNIQUE），原始 Session / commit token 永不落盘、从不
--   进入 Core 响应。
-- - reserved_session_id 在 prepare 时生成并固定，commit 用同一 ID 落正式
--   Human Session 行（session_token 原地激活，Host 不需再从 Core 取秘密）；
--   expires_at 由应用生成（默认 15 分钟），禁止客户端提交。
-- - status 状态机：prepared（active 起止，未消费 Grant）→ committed（原子
--   消费 Grant 并创建正式 Session）或 aborted（Host 主动放弃，Grant 保持
--   active）；`expired` 是应用层按 expires_at 派生的展示状态，不落库。
--   committed / aborted 是终态，不得反向转换或互相转换。事务行不可 DELETE。
--
-- 凭证方向守卫（数据库直接 SQL 也不能伪造）：
-- - 事务签发的 committed Session Metadata 必须与事务 issued_by 同一 Human
--   （复用 home_access 的跨表归属触发器的判定语义）；
-- - commit_token_hash 一经插入不可改写（只经事务摘要授权入口验证）。

CREATE TABLE pairing_transactions (
    transaction_id       TEXT PRIMARY KEY,
    grant_id             TEXT NOT NULL REFERENCES pairing_grants(grant_id),
    client_nonce         TEXT NOT NULL,
    session_token_hash   TEXT NOT NULL UNIQUE CHECK (length(session_token_hash) = 64),
    commit_token_hash    TEXT NOT NULL UNIQUE CHECK (length(commit_token_hash) = 64),
    issued_by            TEXT NOT NULL REFERENCES participants(id),
    reserved_session_id  TEXT NOT NULL UNIQUE,
    status               TEXT NOT NULL
                         CHECK (status IN ('prepared', 'committed', 'aborted')),
    prepared_at          INTEGER NOT NULL,
    expires_at           INTEGER NOT NULL,
    committed_at         INTEGER,
    aborted_at           INTEGER,
    UNIQUE (grant_id, client_nonce, session_token_hash, commit_token_hash),
    CHECK (typeof(transaction_id) = 'text'),
    CHECK (length(transaction_id) = 36),
    CHECK (substr(transaction_id, 9, 1) = '-'),
    CHECK (substr(transaction_id, 14, 1) = '-'),
    CHECK (substr(transaction_id, 19, 1) = '-'),
    CHECK (substr(transaction_id, 24, 1) = '-'),
    CHECK (substr(transaction_id, 15, 1) = '4'),
    CHECK (substr(transaction_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (transaction_id = lower(transaction_id)),
    CHECK (instr(transaction_id, char(0)) = 0),
    CHECK (transaction_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(transaction_id, '-', '')) = 32),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (instr(client_nonce, char(0)) = 0),
    CHECK (length(reserved_session_id) = 36),
    CHECK (substr(reserved_session_id, 9, 1) = '-'),
    CHECK (substr(reserved_session_id, 14, 1) = '-'),
    CHECK (substr(reserved_session_id, 19, 1) = '-'),
    CHECK (substr(reserved_session_id, 24, 1) = '-'),
    CHECK (substr(reserved_session_id, 15, 1) = '4'),
    CHECK (substr(reserved_session_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (reserved_session_id = lower(reserved_session_id)),
    CHECK (instr(reserved_session_id, char(0)) = 0),
    CHECK (reserved_session_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(reserved_session_id, '-', '')) = 32),
    CHECK (expires_at > prepared_at),
    CHECK ((status = 'prepared' AND committed_at IS NULL AND aborted_at IS NULL)
        OR (status = 'committed' AND committed_at IS NOT NULL AND aborted_at IS NULL)
        OR (status = 'aborted' AND aborted_at IS NOT NULL AND committed_at IS NULL))
);

CREATE INDEX idx_pairing_transactions_issued_by ON pairing_transactions(issued_by);
CREATE INDEX idx_pairing_transactions_grant_id ON pairing_transactions(grant_id);
CREATE INDEX idx_pairing_transactions_commit_hash ON pairing_transactions(commit_token_hash);

-- 事务签发的 Session Metadata 必须与事务 issued_by 同一 Human（直接 SQL
-- 也不能伪造跨 Human 的审计来源关系）。
CREATE TRIGGER pairing_transactions_source_grant_same_human
BEFORE INSERT ON pairing_transactions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM human_sessions AS s
    JOIN pairing_grants AS g ON g.grant_id = NEW.grant_id
    WHERE s.id = NEW.reserved_session_id AND s.human_id = g.issued_by
) AND NEW.status = 'committed'
BEGIN
    SELECT RAISE(ABORT, 'pairing transaction source grant must belong to reserved session human');
END;

-- prepare 事实不可改写：识别摘要与时间戳一经插入不可变。
CREATE TRIGGER pairing_transactions_prepare_fields_immutable
BEFORE UPDATE OF transaction_id, grant_id, client_nonce, session_token_hash,
    commit_token_hash, issued_by, reserved_session_id, prepared_at, expires_at
ON pairing_transactions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'pairing transaction prepare fields are immutable');
END;

-- 状态机终态守卫：committed / aborted 都是终态；prepared 只允许一次提交
-- （committed 必须携带 committed_at、不携带 aborted_at）或一次放弃
-- （aborted 必须携带 aborted_at、不携带 committed_at）。
CREATE TRIGGER pairing_transactions_status_terminal
BEFORE UPDATE OF status, committed_at, aborted_at ON pairing_transactions
FOR EACH ROW
WHEN (OLD.status <> 'prepared'
      AND (NEW.status <> OLD.status OR NEW.committed_at IS NOT OLD.committed_at
           OR NEW.aborted_at IS NOT OLD.aborted_at))
  OR (OLD.status = 'prepared' AND NEW.status = 'committed'
      AND (NEW.committed_at IS NULL OR NEW.aborted_at IS NOT NULL))
  OR (OLD.status = 'prepared' AND NEW.status = 'aborted'
      AND (NEW.aborted_at IS NULL OR NEW.committed_at IS NOT NULL))
  OR (OLD.status = 'prepared' AND NEW.status = 'prepared'
      AND (NEW.committed_at IS NOT NULL OR NEW.aborted_at IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'pairing transaction terminal status is immutable');
END;

-- Pairing Transaction 是公共审计事实：prepared/committed/aborted 均不可删。
CREATE TRIGGER pairing_transactions_immutable_delete
BEFORE DELETE ON pairing_transactions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'pairing transaction is immutable');
END;