-- Celira Core V1：Agent Participant 与 Agent Owner 切片。
--
-- 契约：CORE_V1_AGENT_OWNERSHIP.md
-- - participants.kind 受约束地支持 'human' 与 'agent'，status 支持 'active'
--   与 'revoked'；
-- - Agent 的 status 与 revoked_at 必须一致；Human 当前只允许 active 且
--   revoked_at 为空（Human 生命周期撤销不在本切片开放）；
-- - Agent Owner 是独立关系表，两端外键都指向 participants，Human → Agent
--   方向由应用与数据库触发器共同保证：INSERT 与 UPDATE 都校验 human_id
--   指向 Human、agent_id 指向 Agent，且 active Owner 只能指向 active Agent；
--   已 revoked 的历史 Owner 可以继续指向已 revoked 的 Agent（撤销顺序是
--   先 Agent revoked 再 Owner revoked，触发器不阻止该路径）；
-- - 同一 Agent/Human 只能存在一条 Owner 关系（主键约束），不存在两个同时
--   active 的 Owner 关系；
-- - participants.kind 是稳定身份种类，一经插入不可改变（触发器拦截一切
--   改写，防止 Human 与 Agent 身份互换）；
-- - Agent revoked 是当前终态：revoked Agent 不得恢复为 active，原
--   revoked_at 不得改写；重复撤销保留原 revoked_at（触发器拦截）；
--   active → revoked 的一次性正常撤销不受影响；
-- - agent_owners 同样：revoked Owner 不得恢复为 active，原 revoked_at
--   不得改写；插入新的合法 historical revoked Owner 不受影响；
-- - 幂等记录以 (human_id, key) 唯一，关联请求摘要与已创建 Agent；
--   (agent_id, human_id) 复合外键指向 agent_owners，证明该 Human 真正
--   拥有该 Agent（历史幂等记录不要求 Agent active，撤销不删除 Owner 行，
--   Agent 撤销后记录仍然合法）；
-- - 本切片不引入 Backend、Room、Message 或任何延期对象。
--
-- SQLite 无法原位修改 CHECK 约束，因此重建 participants 表：
-- PRAGMA defer_foreign_keys 让外键检查推迟到事务提交，从而可以在同一
-- migration 事务内安全 DROP 旧表；既有 Human、Session 与外键数据原样保留。

PRAGMA defer_foreign_keys = ON;

CREATE TABLE participants_new (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
    status       TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    display_name TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    revoked_at   INTEGER,
    CHECK (length(display_name) <= 80),
    CHECK (instr(display_name, char(0)) = 0),
    CHECK (trim(display_name, char(9) || char(10) || char(13) || ' ') <> ''),
    CHECK (kind <> 'human' OR (status = 'active' AND revoked_at IS NULL)),
    CHECK ((status = 'active' AND revoked_at IS NULL) OR (status = 'revoked' AND revoked_at IS NOT NULL))
);

INSERT INTO participants_new (id, kind, status, display_name, created_at, revoked_at)
    SELECT id, kind, status, display_name, created_at, NULL FROM participants;

DROP TABLE participants;

ALTER TABLE participants_new RENAME TO participants;

-- participants 生命周期守卫：kind 是稳定身份种类，一经插入不可改变；
-- revoked 是当前终态（Agent），不得恢复 active，原 revoked_at 不得改写。
-- active → revoked 的一次性正常撤销（同时写入 revoked_at）不受影响；
-- Human 保持既有 CHECK（只允许 active 且 revoked_at 为空）。

CREATE TRIGGER participants_kind_immutable
BEFORE UPDATE OF kind ON participants
FOR EACH ROW
WHEN NEW.kind <> OLD.kind
BEGIN
    SELECT RAISE(ABORT, 'participant kind is immutable');
END;

CREATE TRIGGER participants_revoked_terminal
BEFORE UPDATE OF status ON participants
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.status = 'active'
BEGIN
    SELECT RAISE(ABORT, 'revoked participant cannot be restored to active');
END;

CREATE TRIGGER participants_revoked_at_immutable
BEFORE UPDATE OF revoked_at ON participants
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.revoked_at IS NOT OLD.revoked_at
BEGIN
    SELECT RAISE(ABORT, 'revoked_at of a revoked participant is immutable');
END;

CREATE TABLE agent_owners (
    agent_id   TEXT NOT NULL REFERENCES participants(id),
    human_id   TEXT NOT NULL REFERENCES participants(id),
    status     TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at INTEGER NOT NULL,
    revoked_at INTEGER,
    PRIMARY KEY (agent_id, human_id),
    CHECK (human_id <> agent_id),
    CHECK ((status = 'active' AND revoked_at IS NULL) OR (status = 'revoked' AND revoked_at IS NOT NULL))
);

CREATE INDEX idx_agent_owners_human ON agent_owners(human_id);

-- agent_owners 守卫：INSERT 与 UPDATE 都保证方向与状态不变量。
-- UPDATE 只针对触碰 agent_id / human_id / status 的语句，纯列数据（如
-- created_at）更新不触发，避免误伤。
-- 撤销路径（先 Agent revoked，再 Owner revoked）不受影响：Owner 变为
-- revoked 时 NEW.status <> 'active'，active-agent 检查跳过。

CREATE TRIGGER agent_owners_insert_direction
BEFORE INSERT ON agent_owners
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'agent_owner direction must be human -> agent');
END;

CREATE TRIGGER agent_owners_update_direction
BEFORE UPDATE OF agent_id, human_id, status ON agent_owners
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'agent_owner direction must be human -> agent');
END;

CREATE TRIGGER agent_owners_insert_active_agent
BEFORE INSERT ON agent_owners
FOR EACH ROW
WHEN NEW.status = 'active'
  AND NOT EXISTS (SELECT 1 FROM participants
                  WHERE id = NEW.agent_id AND kind = 'agent' AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'active agent_owner requires active agent');
END;

CREATE TRIGGER agent_owners_update_active_agent
BEFORE UPDATE OF agent_id, human_id, status ON agent_owners
FOR EACH ROW
WHEN NEW.status = 'active'
  AND NOT EXISTS (SELECT 1 FROM participants
                  WHERE id = NEW.agent_id AND kind = 'agent' AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'active agent_owner requires active agent');
END;

-- agent_owners 生命周期守卫：revoked Owner 是当前终态，不得恢复 active，
-- 原 revoked_at 不得改写；active → revoked 的一次性正常撤销不受影响，
-- INSERT 新历史记录也不受影响（守卫只作用于 UPDATE）。

CREATE TRIGGER agent_owners_revoked_terminal
BEFORE UPDATE OF status ON agent_owners
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.status = 'active'
BEGIN
    SELECT RAISE(ABORT, 'revoked agent_owner cannot be restored to active');
END;

CREATE TRIGGER agent_owners_revoked_at_immutable
BEFORE UPDATE OF revoked_at ON agent_owners
FOR EACH ROW
WHEN OLD.status = 'revoked' AND NEW.revoked_at IS NOT OLD.revoked_at
BEGIN
    SELECT RAISE(ABORT, 'revoked_at of a revoked agent_owner is immutable');
END;

-- agent_create_idempotency 守卫：human_id 必须指向 Human、agent_id 必须指向
-- Agent，且 (agent_id, human_id) 必须是 agent_owners 中真实存在的
-- Human→Agent 拥有关系（复合外键），类型方向触发器只能证明两端身份，
-- 不能证明归属，因此归属以数据库级复合外键为准。
-- 历史幂等记录在 Agent 撤销后仍然合法，因此不要求 Agent active。
-- UPDATE 只针对触碰 human_id / agent_id 的语句。

CREATE TABLE agent_create_idempotency (
    human_id       TEXT NOT NULL REFERENCES participants(id),
    key            TEXT NOT NULL,
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
    agent_id       TEXT NOT NULL REFERENCES participants(id),
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    FOREIGN KEY (agent_id, human_id) REFERENCES agent_owners(agent_id, human_id),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER agent_create_idempotency_insert_human_direction
BEFORE INSERT ON agent_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
BEGIN
    SELECT RAISE(ABORT, 'idempotency human_id must reference a human');
END;

CREATE TRIGGER agent_create_idempotency_update_human_direction
BEFORE UPDATE OF human_id ON agent_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
BEGIN
    SELECT RAISE(ABORT, 'idempotency human_id must reference a human');
END;

CREATE TRIGGER agent_create_idempotency_insert_agent_direction
BEFORE INSERT ON agent_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'idempotency agent_id must reference an agent');
END;

CREATE TRIGGER agent_create_idempotency_update_agent_direction
BEFORE UPDATE OF agent_id ON agent_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'idempotency agent_id must reference an agent');
END;

PRAGMA defer_foreign_keys = OFF;
