-- Celira Core V1：Human 身份与认证切片。
--
-- 契约：CORE_V1_IDENTITY_AUTH.md
-- - Participant 与 human_sessions 分表持有，外键指向明确；
-- - kind / status 使用受约束枚举值（当前只有 human / active）；
-- - display name 由应用层校验，SQLite CHECK 对 U+0000、空值和长度做最终兜底；
-- - token hash 唯一且固定长度（SHA-256 十六进制，64 字符）；
-- - 原始 token 永不落盘（本表只存摘要）；
-- - bootstrap guard 以数据库级主键约束保证全库唯一（id 只能等于 1），
--   guard、首位 Human 与初始 Session 必须在同一事务内提交。

CREATE TABLE participants (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('human')),
    status       TEXT NOT NULL CHECK (status IN ('active')),
    display_name TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    CHECK (length(display_name) <= 80),
    CHECK (instr(display_name, char(0)) = 0),
    CHECK (trim(display_name, char(9) || char(10) || char(13) || ' ') <> '')
);

CREATE TABLE human_sessions (
    id         TEXT PRIMARY KEY,
    human_id   TEXT NOT NULL REFERENCES participants(id),
    token_hash TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);

CREATE TABLE bootstrap_guard (
    id INTEGER PRIMARY KEY CHECK (id = 1)
);
