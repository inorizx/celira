-- Celira Core V1：Backend Binding 与 Backend Credential 切片。
--
-- 契约：CORE_V1_BACKEND_BINDINGS.md
-- - backend_bindings 是 Agent 与获准驱动它的外部 Backend 之间的长期授权
--   连接：稳定 Binding ID（UUID v4）、Agent Participant、创建授权的 Human、
--   Human 可读 label、status、created_at 与可空 revoked_at；一个 Agent 同时
--   最多一个 active Binding（partial unique index）；创建时 created_by 指向
--   active Human、Agent 指向 active Agent、存在 active Owner 关系，且
--   (agent_id, created_by_human_id) 复合外键证明绑定关联真实 Agent Owner
--   历史；revoked 是终态，不可恢复、不可硬删除、原 revoked_at 不可改写；
--   Binding 身份（id/agent/created_by/label）、幂等协调列与 created_at
--   不可改写；幂等协调列随事实行保存，不建立第二张幂等事实表；
-- - backend_credentials 是 Binding 下的一次可撤销秘密能力：稳定 Credential
--   ID（UUID v4）、所属 Binding、签发 Human、不可恢复的 token hash、可空
--   replaces ID、status、created_at 与可空 revoked_at；一个 Binding 同时
--   最多一个 active Credential（partial unique index）；token hash 全局唯一、
--   固定 64 个小写 hex；replaces ID 指向同一 Binding 的历史 Credential，
--   不能自指；revoked 是终态，不恢复、不硬删除、不改写原 revoked_at；
--   Credential 身份（id/binding/issuer/hash/replaces）、签发幂等协调列与
--   created_at 不可改写；
-- - Agent revoke 传播：在既有 participants AFTER UPDATE 触发器（004 的
--   Membership 传播）基础上，本迁移新增同一事务内的 Credential 与 Binding
--   传播触发器，用 NEW.revoked_at 将该 Agent 的全部 active Credential 与
--   active Binding 置为 revoked；重复撤销（已 revoked）不触发；
-- - 撤销传播方向恒为：先撤销 active Credential，再撤销 active Binding；
--   Binding 撤销守卫拒绝在仍遗留 active Credential 时置为 revoked；
-- - 本迁移不引入 Event、Completion、ACK、Cursor、Lease、Runtime、Provider、
--   Memory、Tool、MCP、Backend Participant 或 Backend 注册表对象。

CREATE TABLE backend_bindings (
    id                  TEXT PRIMARY KEY,
    agent_id            TEXT NOT NULL REFERENCES participants(id),
    created_by_human_id TEXT NOT NULL REFERENCES participants(id),
    label               TEXT NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at          INTEGER NOT NULL,
    revoked_at          INTEGER,
    idempotency_key     TEXT NOT NULL,
    CHECK (typeof(label) = 'text'),
    CHECK (length(label) <= 80),
    CHECK (instr(label, char(0)) = 0),
    -- 与 Rust char::is_whitespace() 的 Unicode White_Space 集合保持一致；
    -- SQLite 默认 trim 只识别 ASCII space，必须显式列出其余字符。
    CHECK (trim(label,
        char(9) || char(10) || char(11) || char(12) || char(13) || char(32) ||
        char(133) || char(160) || char(5760) ||
        char(8192) || char(8193) || char(8194) || char(8195) || char(8196) ||
        char(8197) || char(8198) || char(8199) || char(8200) || char(8201) ||
        char(8202) || char(8232) || char(8233) || char(8239) || char(8287) ||
        char(12288)
    ) <> ''),
    CHECK ((status = 'active' AND revoked_at IS NULL) OR (status = 'revoked' AND revoked_at IS NOT NULL)),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*'),
    UNIQUE (created_by_human_id, idempotency_key),
    FOREIGN KEY (agent_id, created_by_human_id) REFERENCES agent_owners(agent_id, human_id)
);

-- 一个 Agent 同时最多一个 active Binding；partial unique index 允许保留
-- 多个历史（revoked）Binding。
CREATE UNIQUE INDEX idx_backend_bindings_one_active_per_agent
    ON backend_bindings(agent_id) WHERE status = 'active';

-- 新 Binding 只能是 active 且无 revoked_at：revoked 只能由 UPDATE 路径
-- （显式撤销或 Agent revoke 传播）产生。
CREATE TRIGGER backend_bindings_insert_active_only
BEFORE INSERT ON backend_bindings
FOR EACH ROW
WHEN NEW.status <> 'active' OR NEW.revoked_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new backend bindings must be active with no revoked_at');
END;

-- 方向：created_by 必须指向 Human，agent 必须指向 Agent。
CREATE TRIGGER backend_bindings_insert_direction
BEFORE INSERT ON backend_bindings
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.created_by_human_id AND kind = 'human')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'backend binding direction must be human -> agent');
END;

-- active Binding 只允许指向 active Human + active Agent + 存在 active
-- Owner 关系（复合外键已证明 Owner 历史真实存在，此处证明关系当前 active）。
CREATE TRIGGER backend_bindings_insert_active_guard
BEFORE INSERT ON backend_bindings
FOR EACH ROW
WHEN NEW.status = 'active'
  AND NOT EXISTS (
      SELECT 1 FROM participants h, participants a, agent_owners o
      WHERE h.id = NEW.created_by_human_id AND h.kind = 'human' AND h.status = 'active'
        AND a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active'
        AND o.agent_id = NEW.agent_id AND o.human_id = NEW.created_by_human_id AND o.status = 'active'
  )
BEGIN
    SELECT RAISE(ABORT, 'active backend binding requires active human, active agent and active owner');
END;

-- Binding 身份（id/agent/created_by/label）、幂等协调列与 created_at 不可改写。
CREATE TRIGGER backend_bindings_identity_immutable
BEFORE UPDATE OF id, agent_id, created_by_human_id, label, idempotency_key, created_at
ON backend_bindings
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.created_by_human_id IS NOT OLD.created_by_human_id
  OR NEW.label IS NOT OLD.label
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'backend binding identity is immutable');
END;

-- revoked 是终态：不得恢复 active，原 revoked_at 不得改写。
CREATE TRIGGER backend_bindings_revoked_terminal
BEFORE UPDATE OF status ON backend_bindings
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.status = 'active'
BEGIN
    SELECT RAISE(ABORT, 'revoked backend binding cannot be restored to active');
END;

CREATE TRIGGER backend_bindings_revoked_at_immutable
BEFORE UPDATE OF revoked_at ON backend_bindings
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.revoked_at IS NOT OLD.revoked_at
BEGIN
    SELECT RAISE(ABORT, 'revoked_at of a revoked backend binding is immutable');
END;

-- 撤销顺序守卫：active Binding 置为 revoked 前必须已撤销其全部 active
-- Credential（撤销 Binding 与撤销 Agent 的传播路径都在同一事务内先撤销
-- Credential）。
CREATE TRIGGER backend_bindings_revoke_requires_no_active_credentials
BEFORE UPDATE OF status ON backend_bindings
FOR EACH ROW
WHEN OLD.status = 'active' AND NEW.status = 'revoked'
  AND EXISTS (SELECT 1 FROM backend_credentials WHERE binding_id = NEW.id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'binding revoke requires its active credentials revoked first');
END;

-- Binding 不允许硬删除。
CREATE TRIGGER backend_bindings_no_delete
BEFORE DELETE ON backend_bindings
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'backend bindings cannot be deleted');
END;

CREATE TABLE backend_credentials (
    id                     TEXT PRIMARY KEY,
    binding_id             TEXT NOT NULL REFERENCES backend_bindings(id),
    issued_by_human_id     TEXT NOT NULL REFERENCES participants(id),
    token_hash             TEXT NOT NULL UNIQUE,
    replaces_credential_id TEXT,
    status                 TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at             INTEGER NOT NULL,
    revoked_at             INTEGER,
    idempotency_key        TEXT NOT NULL,
    CHECK (typeof(token_hash) = 'text'),
    CHECK (length(token_hash) = 64),
    CHECK (token_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (token_hash = lower(token_hash)),
    CHECK (replaces_credential_id IS NULL OR replaces_credential_id <> id),
    CHECK ((status = 'active' AND revoked_at IS NULL) OR (status = 'revoked' AND revoked_at IS NOT NULL)),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*'),
    UNIQUE (issued_by_human_id, idempotency_key),
    FOREIGN KEY (replaces_credential_id) REFERENCES backend_credentials(id)
);

CREATE INDEX idx_backend_credentials_binding ON backend_credentials(binding_id);

-- 一个 Binding 同时最多一个 active Credential；partial unique index 允许
-- 保留多个历史（revoked）Credential。
CREATE UNIQUE INDEX idx_backend_credentials_one_active_per_binding
    ON backend_credentials(binding_id) WHERE status = 'active';

-- 新 Credential 只能是 active 且无 revoked_at：revoked 只能由 UPDATE 路径
-- （显式撤销、轮换交接或上级生命周期传播）产生。
CREATE TRIGGER backend_credentials_insert_active_only
BEFORE INSERT ON backend_credentials
FOR EACH ROW
WHEN NEW.status <> 'active' OR NEW.revoked_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new backend credentials must be active with no revoked_at');
END;

-- 签发人必须指向 Human。
CREATE TRIGGER backend_credentials_insert_direction
BEFORE INSERT ON backend_credentials
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.issued_by_human_id AND kind = 'human')
BEGIN
    SELECT RAISE(ABORT, 'backend credential issuer must be a human');
END;

-- active Credential 只允许指向 active Binding（该 Binding 指向 active
-- Agent）、active 签发 Human，且签发 Human 对该 Agent 的 Owner 关系当前
-- active。
CREATE TRIGGER backend_credentials_insert_active_guard
BEFORE INSERT ON backend_credentials
FOR EACH ROW
WHEN NEW.status = 'active'
  AND NOT EXISTS (
      SELECT 1
      FROM backend_bindings b
      JOIN participants a ON a.id = b.agent_id
      JOIN participants h ON h.id = NEW.issued_by_human_id
      JOIN agent_owners o ON o.agent_id = b.agent_id AND o.human_id = NEW.issued_by_human_id
      WHERE b.id = NEW.binding_id
        AND b.status = 'active'
        AND a.kind = 'agent' AND a.status = 'active'
        AND h.status = 'active'
        AND o.status = 'active'
  )
BEGIN
    SELECT RAISE(ABORT, 'active backend credential requires active binding, active agent, active issuer and active owner');
END;

-- 轮换是数据库级 CAS：replaces ID 必须正是同一 Binding 当前 active
-- Credential。同一个 BEFORE INSERT 触发器内先校验、再用 NEW.created_at
-- 撤销旧事实，不能依赖多个同类触发器的执行顺序；若后续 INSERT 的任何
-- 约束失败，SQLite 会连同该 UPDATE 一起回滚。
CREATE TRIGGER backend_credentials_rotate_current_active
BEFORE INSERT ON backend_credentials
FOR EACH ROW
WHEN NEW.replaces_credential_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM backend_credentials c
         WHERE c.id = NEW.replaces_credential_id
           AND c.binding_id = NEW.binding_id
           AND c.status = 'active'
    ) THEN RAISE(ABORT, 'replaces_credential_id must be the current active credential') END;

    UPDATE backend_credentials
       SET status = 'revoked', revoked_at = NEW.created_at
     WHERE id = NEW.replaces_credential_id
       AND binding_id = NEW.binding_id
       AND status = 'active';
END;

-- Credential 身份（id/binding/issuer/hash/replaces）、签发幂等协调列与
-- created_at 不可改写。
CREATE TRIGGER backend_credentials_identity_immutable
BEFORE UPDATE OF id, binding_id, issued_by_human_id, token_hash, replaces_credential_id,
                idempotency_key, created_at
ON backend_credentials
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
  OR NEW.binding_id IS NOT OLD.binding_id
  OR NEW.issued_by_human_id IS NOT OLD.issued_by_human_id
  OR NEW.token_hash IS NOT OLD.token_hash
  OR NEW.replaces_credential_id IS NOT OLD.replaces_credential_id
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'backend credential identity is immutable');
END;

-- revoked 是终态：不得恢复 active，原 revoked_at 不得改写。
CREATE TRIGGER backend_credentials_revoked_terminal
BEFORE UPDATE OF status ON backend_credentials
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.status = 'active'
BEGIN
    SELECT RAISE(ABORT, 'revoked backend credential cannot be restored to active');
END;

CREATE TRIGGER backend_credentials_revoked_at_immutable
BEFORE UPDATE OF revoked_at ON backend_credentials
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.revoked_at IS NOT OLD.revoked_at
BEGIN
    SELECT RAISE(ABORT, 'revoked_at of a revoked backend credential is immutable');
END;

-- Credential 不允许硬删除。
CREATE TRIGGER backend_credentials_no_delete
BEFORE DELETE ON backend_credentials
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'backend credentials cannot be deleted');
END;

-- Agent revoke 传播（本迁移新增，不改动 004 的 Membership 传播）：Agent
-- 变为 revoked 的同一事务内，先撤销该 Agent 全部 active Credential，再
-- 撤销其全部 active Binding，全部使用 NEW.revoked_at；重复撤销（已
-- revoked）不触发本触发器。
CREATE TRIGGER agent_revoke_propagates_backend_bindings_and_credentials
AFTER UPDATE OF status ON participants
FOR EACH ROW
WHEN OLD.kind = 'agent' AND OLD.status = 'active' AND NEW.status = 'revoked'
BEGIN
    UPDATE backend_credentials
       SET status = 'revoked', revoked_at = NEW.revoked_at
     WHERE binding_id IN (SELECT id FROM backend_bindings WHERE agent_id = NEW.id AND status = 'active')
       AND status = 'active';
    UPDATE backend_bindings
       SET status = 'revoked', revoked_at = NEW.revoked_at
     WHERE agent_id = NEW.id AND status = 'active';
END;
