-- Celira Core V1：Agent Service P0 Phase 1（Publication 版本链 + Access 关系 +
-- Service Direct Room Binding + 幂等记录）。
--
-- 契约：CORE_V1_AGENT_SERVICE.md（唯一权威契约）
-- 本迁移只新增本切片公共事实，并重建两个既有守卫触发器（
-- direct_room_pairs_insert_active_owner、agent_events_insert_guard）以加入
-- Service 分支；其余既有冻结语义（messages/memberships/events/completions/
-- acks/permissions/backend bindings/credentials 等）一律不动，不生成任何
-- Message/Human Event/Agent Event/Completion/ACK/Membership/Permission/
-- Binding/Credential（由 migration 完整性测试以计数/schema 证明）。
--
-- 事实模型：
-- - agent_service_publication_versions：Agent 的不可变 Publication 版本链。
--   PK(agent_id, publication_version)；版本 > 0 且同 Agent 严格单调 +1
--   （触发器）；不可 UPDATE/DELETE（REPLACE 以 BEFORE INSERT identity guard
--   拒绝）；当前状态永远是最大版本行，不存第二张 current 表；最新 publication
--   的存在性/可见性/状态不复制到 Access 行。
-- - agent_service_accesses：(human, agent) 的可撤销公共关系。active ->
--   revoked 是唯一 UPDATE；已 revoked 不得恢复；grant 身份字段不可改；不得
--   DELETE/REPLACE；同 (agent, human) 至多一个 active（partial unique index）；
--   Agent revoked 时触发器在同一事务内把其全部 active Access 置 revoked
--   （reason agent_revoked、at NEW.revoked_at、by NULL），不破坏既有 agent
--   revoke/Membership 传播（既有触发器原样保留，本迁移只新增）。
-- - service_direct_room_bindings：把 room_id 绑定到一份 Access，并以 human/
--   agent 结构见证同时与 access 和 direct pair 一致；一个 Access 可绑定多个
--   Room，任何 Room 至多一个 Binding；Binding 不可 UPDATE/DELETE/REPLACE。
-- - 幂等记录：publication_put / access_grant / direct_room_create 三张；
--   只存 SHA-256 请求摘要与结果标识，绝不复制 service 文本；deferred 复合
--   外键与见证触发器只证明引用/身份一致性（result 指向精确事实行、actor
--   与时间一致、result_is_create == (版本==1)），同事务原子性由 Phase 2
--   adapter 的单个 `BEGIN IMMEDIATE` 保证。
--
-- 文本规则（同 Participant Presentation）：空串由应用规范化为 NULL；SQL 层
-- 同样拒绝 ''（text 列 <> '' CHECK）；纯空白保留不 trim；禁 U+0000
-- （instr(value, char(0)) = 0）；字节上限用 length(CAST(value AS BLOB)) 裁决。
--
-- 时间戳由 Core 生成（published_at/granted_at/binding created_at/幂等
-- created_at），客户端不得指定。
--
-- 表创建顺序：先建六张新表，再重建两个既有守卫触发器（其正文引用本迁移
-- 已创建的表；SQLite 触发器正文在 DML 时才解析表引用）。

-- ============================================================
-- 1. Agent Service Publication 版本链
-- ============================================================

CREATE TABLE agent_service_publication_versions (
    agent_id             TEXT NOT NULL REFERENCES participants(id),
    publication_version  INTEGER NOT NULL CHECK (publication_version > 0),
    listing_visibility   TEXT NOT NULL CHECK (listing_visibility IN ('unpublished', 'invite_only')),
    service_status       TEXT NOT NULL CHECK (service_status IN ('accepting', 'suspended')),
    service_summary      TEXT,
    service_description  TEXT,
    service_disclaimer   TEXT,
    published_by_id      TEXT NOT NULL REFERENCES participants(id),
    published_at         INTEGER NOT NULL,
    PRIMARY KEY (agent_id, publication_version),
    -- 文本卫生：NUL 拒绝、BLOB 字节上限、纯空白的非空串合法（不 trim）。
    CHECK (service_summary IS NULL
        OR (typeof(service_summary) = 'text'
            AND instr(service_summary, char(0)) = 0
            AND service_summary <> ''
            AND length(CAST(service_summary AS BLOB)) <= 512)),
    CHECK (service_description IS NULL
        OR (typeof(service_description) = 'text'
            AND instr(service_description, char(0)) = 0
            AND service_description <> ''
            AND length(CAST(service_description AS BLOB)) <= 16384)),
    CHECK (service_disclaimer IS NULL
        OR (typeof(service_disclaimer) = 'text'
            AND instr(service_disclaimer, char(0)) = 0
            AND service_disclaimer <> ''
            AND length(CAST(service_disclaimer AS BLOB)) <= 4096)),
    -- invite_only 版本必须有 service_summary；unpublished 可无。
    CHECK (listing_visibility <> 'invite_only' OR service_summary IS NOT NULL),
    -- 其余类型（数字/JSON/blob）即使满足字节数也不允许：显式收口为 text/NULL。
    CHECK (service_summary IS NULL OR typeof(service_summary) = 'text'),
    CHECK (service_description IS NULL OR typeof(service_description) = 'text'),
    CHECK (service_disclaimer IS NULL OR typeof(service_disclaimer) = 'text')
);

CREATE TRIGGER agent_service_publication_versions_immutable
BEFORE UPDATE ON agent_service_publication_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service publication versions are immutable');
END;

CREATE TRIGGER agent_service_publication_versions_no_delete
BEFORE DELETE ON agent_service_publication_versions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service publication versions cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键冲突时先隐式删除冲突行，可绕过 immutable /
-- no-delete 守卫改写或重建既有版本。
CREATE TRIGGER agent_service_publication_versions_insert_identity_guard
BEFORE INSERT ON agent_service_publication_versions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_service_publication_versions
    WHERE agent_id = NEW.agent_id
      AND publication_version = NEW.publication_version
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication version identity already exists');
END;

-- 版本严格单调：新版本必须等于该 Agent 当前最大版本 + 1（无行 => 1）。
-- 缺号/跳号/重复一律拒绝；base_version 的并发裁决依赖此不变量。
CREATE TRIGGER agent_service_publication_version_strictly_increments
BEFORE INSERT ON agent_service_publication_versions
FOR EACH ROW
WHEN NEW.publication_version <> (
    SELECT COALESCE(MAX(publication_version), 0) + 1
    FROM agent_service_publication_versions
    WHERE agent_id = NEW.agent_id
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication version must be exactly one more than the latest');
END;

-- 发布资格（database 兜底）：发布者必须是 active Human 且是 Active Agent 的
-- active Owner；target 必须是 active Agent。
CREATE TRIGGER agent_service_publication_versions_publisher_guard
BEFORE INSERT ON agent_service_publication_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.published_by_id AND p.kind = 'human' AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants a
    WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM agent_owners ao
    WHERE ao.agent_id = NEW.agent_id AND ao.human_id = NEW.published_by_id AND ao.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication requires an active agent owned by an active human publisher');
END;

CREATE INDEX idx_agent_service_publication_versions_agent
    ON agent_service_publication_versions(agent_id, publication_version DESC);

-- ============================================================
-- 2. Agent Service Access 关系
-- ============================================================

CREATE TABLE agent_service_accesses (
    access_id            TEXT PRIMARY KEY,
    agent_id             TEXT NOT NULL REFERENCES participants(id),
    human_id             TEXT NOT NULL REFERENCES participants(id),
    source               TEXT NOT NULL CHECK (source IN ('owner_granted')),
    status               TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    granted_by_human_id  TEXT NOT NULL REFERENCES participants(id),
    granted_at           INTEGER NOT NULL,
    revocation_reason    TEXT CHECK (revocation_reason IS NULL
                                    OR revocation_reason IN ('owner_revoked', 'agent_revoked')),
    revoked_by_human_id  TEXT REFERENCES participants(id),
    revoked_at           INTEGER,
    CHECK (typeof(access_id) = 'text'),
    CHECK (length(access_id) = 36),
    CHECK (substr(access_id, 9, 1) = '-'),
    CHECK (substr(access_id, 14, 1) = '-'),
    CHECK (substr(access_id, 19, 1) = '-'),
    CHECK (substr(access_id, 24, 1) = '-'),
    CHECK (substr(access_id, 15, 1) = '4'),
    CHECK (substr(access_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (access_id = lower(access_id)),
    CHECK (instr(access_id, char(0)) = 0),
    CHECK (access_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(access_id, '-', '')) = 32),
    CHECK (human_id <> agent_id),
    CHECK (human_id <> granted_by_human_id),
    CHECK (agent_id <> granted_by_human_id),
    -- 状态三元组一致性：active 时三个 revoked 字段全 NULL；revoked 时
    -- reason 与 revoked_at 非 NULL（revoked_by 的合法性由 revoke 状态机
    -- 触发器按 reason 分支裁决）。
    CHECK ((status = 'active' AND revocation_reason IS NULL
            AND revoked_by_human_id IS NULL AND revoked_at IS NULL)
        OR (status = 'revoked' AND revocation_reason IS NOT NULL AND revoked_at IS NOT NULL)),
    -- grant 幂等记录 deferred 复合外键的 parent key。
    UNIQUE (access_id, granted_by_human_id, granted_at)
);

CREATE INDEX idx_agent_service_accesses_agent ON agent_service_accesses(agent_id);
CREATE INDEX idx_agent_service_accesses_human ON agent_service_accesses(human_id);

-- 同 (agent, human) 至多一个 active Access：partial unique index（revoked
-- 历史行不占位）。首次成功授予后，regrant 必须先撤销，再签新 access_id。
CREATE UNIQUE INDEX ux_agent_service_accesses_single_active
    ON agent_service_accesses(agent_id, human_id)
    WHERE status = 'active';

-- 新 Access 必须是 active（无 revoked 字段）：revoked 只能由 UPDATE 路径
-- 产生（owner 主动撤销或 Agent revoke 传播），不能直接插入、不能经 REPLACE
-- 复活；revoked 行的创建只能来自合法迁移。
CREATE TRIGGER agent_service_accesses_insert_active_only
BEFORE INSERT ON agent_service_accesses
FOR EACH ROW
WHEN NEW.status <> 'active'
  OR NEW.revocation_reason IS NOT NULL
  OR NEW.revoked_by_human_id IS NOT NULL
  OR NEW.revoked_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new agent service accesses must be active with no revoked fields');
END;

-- 插入资格（database 兜底，方向/关系/可见性）：granted_by 是 active Human
-- 且为 active Agent 的 active Owner；agent 是 active Agent；human 是 active
-- Human 且 ≠ granted_by；human 与 granted_by 为 accepted Contact；最新
-- Publication 必存在且 listing_visibility='invite_only'。service_status 可为
-- suspended（暂停期间预先邀请合法，使用阶段才被拒）。
CREATE TRIGGER agent_service_accesses_insert_guard
BEFORE INSERT ON agent_service_accesses
FOR EACH ROW
WHEN NOT (
    NEW.status = 'active'
    AND EXISTS (SELECT 1 FROM participants a
                WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active')
    AND EXISTS (SELECT 1 FROM participants h
                WHERE h.id = NEW.human_id AND h.kind = 'human' AND h.status = 'active')
    AND EXISTS (SELECT 1 FROM participants g
                WHERE g.id = NEW.granted_by_human_id AND g.kind = 'human' AND g.status = 'active')
    AND EXISTS (SELECT 1 FROM agent_owners ao
                WHERE ao.agent_id = NEW.agent_id AND ao.human_id = NEW.granted_by_human_id
                  AND ao.status = 'active')
    AND EXISTS (SELECT 1 FROM contacts c
                WHERE ((c.a_id = NEW.granted_by_human_id AND c.b_id = NEW.human_id)
                    OR (c.a_id = NEW.human_id AND c.b_id = NEW.granted_by_human_id))
                  AND c.status = 'accepted')
    AND EXISTS (
        SELECT 1 FROM agent_service_publication_versions v
        WHERE v.agent_id = NEW.agent_id
          AND v.publication_version = (
              SELECT MAX(v2.publication_version)
              FROM agent_service_publication_versions v2
              WHERE v2.agent_id = NEW.agent_id)
          AND v.listing_visibility = 'invite_only'
    )
)
BEGIN
    SELECT RAISE(ABORT, 'service access requires an active agent with an invite_only publication granted to an accepted contact by the active owner');
END;

-- grant 身份字段（access_id/agent_id/human_id/source/granted_by_human_id/
-- granted_at）不可改写。
CREATE TRIGGER agent_service_accesses_identity_immutable
BEFORE UPDATE OF access_id, agent_id, human_id, source, granted_by_human_id, granted_at
ON agent_service_accesses
FOR EACH ROW
WHEN NEW.access_id IS NOT OLD.access_id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.source IS NOT OLD.source
  OR NEW.granted_by_human_id IS NOT OLD.granted_by_human_id
  OR NEW.granted_at IS NOT OLD.granted_at
BEGIN
    SELECT RAISE(ABORT, 'agent service access grant identity is immutable');
END;

-- 状态机：active -> revoked 是唯一合法 UPDATE。owner_revoked 的 revoker 必须
-- 是 current active Human Owner；agent_revoked 的 revoker 必须为 NULL（由
-- Agent revoke 传播触发器填充）。已 revoked 是终态：任何改动（含改写 revoked
-- 字段或恢复 active）都被拒绝；等值自赋值不触发（SQLite 对 UPDATE OF 列仅在
-- 值实际变化时触发）。
CREATE TRIGGER agent_service_accesses_revoke_state_machine
BEFORE UPDATE OF status, revocation_reason, revoked_by_human_id, revoked_at
ON agent_service_accesses
FOR EACH ROW
WHEN NOT (
    OLD.status = 'active' AND NEW.status = 'revoked'
    AND NEW.revocation_reason IN ('owner_revoked', 'agent_revoked')
    AND NEW.revoked_at IS NOT NULL
    AND ((NEW.revocation_reason = 'owner_revoked'
          AND NEW.revoked_by_human_id IS NOT NULL
          AND EXISTS (SELECT 1 FROM participants p
                      WHERE p.id = NEW.revoked_by_human_id AND p.kind = 'human' AND p.status = 'active')
          AND EXISTS (SELECT 1 FROM agent_owners ao
                      WHERE ao.agent_id = NEW.agent_id AND ao.human_id = NEW.revoked_by_human_id
                        AND ao.status = 'active'))
         OR (NEW.revocation_reason = 'agent_revoked'
             AND NEW.revoked_by_human_id IS NULL))
)
OR OLD.status = 'revoked'
BEGIN
    SELECT RAISE(ABORT, 'service access status can only move once from active to revoked with a consistent revocation tuple');
END;

CREATE TRIGGER agent_service_accesses_no_delete
BEFORE DELETE ON agent_service_accesses
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service accesses cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (access_id) 或部分唯一索引 (agent_id, human_id
-- WHERE status='active') 冲突时先隐式删除冲突行，可绕过 no-delete / immutable
-- / 状态机守卫重建 active 或改写 grant 身份；本守卫在 INSERT 阶段直接拒绝任一
-- 身份已占用的写入（普通重复 INSERT 同样被拒）。
CREATE TRIGGER agent_service_accesses_insert_identity_guard
BEFORE INSERT ON agent_service_accesses
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_service_accesses
    WHERE access_id = NEW.access_id
       OR (agent_id = NEW.agent_id AND human_id = NEW.human_id
           AND NEW.status = 'active' AND status = 'active')
)
BEGIN
    SELECT RAISE(ABORT, 'agent service access identity already exists');
END;

-- Agent revoke 传播：Agent 变为 revoked 的同一事务内，用 NEW.revoked_at 将该
-- Agent 的全部 active Access 置 revoked（reason=agent_revoked、by NULL）。
-- 与既有 agent_revoke_propagates_room_memberships 触发器并存，不破坏其行为；
-- 重复撤销（已 revoked）不触发；revocation 状态机触发器放行本路径
-- （agent_revoked + by NULL + revoked_at=NEW.revoked_at 非 NULL（participants
-- 的 revoked 状态 CHECK 保证））。
CREATE TRIGGER agent_service_accesses_agent_revoke_propagates
AFTER UPDATE OF status ON participants
FOR EACH ROW
WHEN OLD.kind = 'agent' AND OLD.status = 'active' AND NEW.status = 'revoked'
BEGIN
    UPDATE agent_service_accesses
       SET status = 'revoked',
           revocation_reason = 'agent_revoked',
           revoked_by_human_id = NULL,
           revoked_at = NEW.revoked_at
     WHERE agent_id = NEW.id AND status = 'active';
END;

-- ============================================================
-- 3. Publication 幂等记录
-- ============================================================

CREATE TABLE agent_service_publication_put_idempotency (
    actor_human_id             TEXT NOT NULL REFERENCES participants(id),
    agent_id                   TEXT NOT NULL REFERENCES participants(id),
    key                        TEXT NOT NULL,
    request_digest             TEXT NOT NULL CHECK (length(request_digest) = 64),
    result_publication_version INTEGER NOT NULL,
    result_is_create           INTEGER NOT NULL CHECK (result_is_create IN (0, 1)),
    created_at                 INTEGER NOT NULL,
    PRIMARY KEY (actor_human_id, agent_id, key),
    -- deferred 复合外键：result_publication_version 指向精确版本行。只证明
    -- **引用一致性**，不证明同事务原子插入（由 adapter 的 BEGIN IMMEDIATE
    -- 保证，见文件头部注释与 CORE_V1_AGENT_SERVICE.md §7/§8）。
    FOREIGN KEY (agent_id, result_publication_version)
        REFERENCES agent_service_publication_versions(agent_id, publication_version)
        DEFERRABLE INITIALLY DEFERRED,
    -- identity 见证：result_is_create 必须恰好反映 result_publication_version == 1。
    CHECK (result_is_create = (result_publication_version = 1)),
    CHECK (request_digest = lower(request_digest)),
    CHECK (instr(request_digest, char(0)) = 0),
    CHECK (request_digest NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER agent_service_publication_put_idempotency_actor_human
BEFORE INSERT ON agent_service_publication_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication idempotency actor must be an active human');
END;

CREATE TRIGGER agent_service_publication_put_idempotency_immutable
BEFORE UPDATE ON agent_service_publication_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service publication idempotency records are immutable');
END;

CREATE TRIGGER agent_service_publication_put_idempotency_no_delete
BEFORE DELETE ON agent_service_publication_put_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service publication idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键冲突时先隐式删除冲突行，可绕过 no-delete /
-- immutable 守卫重映射 key 到另一请求/版本。
CREATE TRIGGER agent_service_publication_put_idempotency_insert_identity_guard
BEFORE INSERT ON agent_service_publication_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_service_publication_put_idempotency
    WHERE actor_human_id = NEW.actor_human_id
      AND agent_id = NEW.agent_id
      AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication idempotency identity already exists');
END;

-- 见证一致性：被引用版本行的发布者必须等于 actor，published_at 必须等于
-- created_at（同一 Core 操作时间）。只证明身份/引用一致性，不证明同事务。
CREATE TRIGGER agent_service_publication_put_idempotency_witness_consistency
BEFORE INSERT ON agent_service_publication_put_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_service_publication_versions v
    WHERE v.agent_id = NEW.agent_id
      AND v.publication_version = NEW.result_publication_version
      AND (v.published_by_id <> NEW.actor_human_id OR v.published_at <> NEW.created_at)
)
BEGIN
    SELECT RAISE(ABORT, 'agent service publication idempotency witness must match the result version publisher and time');
END;

-- ============================================================
-- 4. Access grant 幂等记录
-- ============================================================

CREATE TABLE agent_service_access_grant_idempotency (
    granted_by_human_id TEXT NOT NULL REFERENCES participants(id),
    key                 TEXT NOT NULL,
    request_digest      TEXT NOT NULL CHECK (length(request_digest) = 64),
    result_access_id    TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    PRIMARY KEY (granted_by_human_id, key),
    -- deferred 复合外键：指向精确 Access（access_id + 授予者 + 时间）。只证明
    -- 引用一致性，不证明同事务原子插入；agent/human 对由请求摘要捕获
    -- （account digest 的 {agent_id, human_id}），不在此冗余副本。
    FOREIGN KEY (result_access_id, granted_by_human_id, created_at)
        REFERENCES agent_service_accesses(access_id, granted_by_human_id, granted_at)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (request_digest = lower(request_digest)),
    CHECK (instr(request_digest, char(0)) = 0),
    CHECK (request_digest NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER agent_service_access_grant_idempotency_actor_human
BEFORE INSERT ON agent_service_access_grant_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.granted_by_human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'agent service access grant idempotency actor must be an active human');
END;

CREATE TRIGGER agent_service_access_grant_idempotency_immutable
BEFORE UPDATE ON agent_service_access_grant_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service access grant idempotency records are immutable');
END;

CREATE TRIGGER agent_service_access_grant_idempotency_no_delete
BEFORE DELETE ON agent_service_access_grant_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent service access grant idempotency records cannot be deleted');
END;

CREATE TRIGGER agent_service_access_grant_idempotency_insert_identity_guard
BEFORE INSERT ON agent_service_access_grant_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_service_access_grant_idempotency
    WHERE granted_by_human_id = NEW.granted_by_human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'agent service access grant idempotency identity already exists');
END;

-- ============================================================
-- 5. Service Direct Room Binding
-- ============================================================

CREATE TABLE service_direct_room_bindings (
    room_id    TEXT PRIMARY KEY,
    access_id  TEXT NOT NULL REFERENCES agent_service_accesses(access_id),
    human_id   TEXT NOT NULL REFERENCES participants(id),
    agent_id   TEXT NOT NULL REFERENCES participants(id),
    created_at INTEGER NOT NULL,
    CHECK (human_id <> agent_id),
    -- 幂等记录 deferred 复合外键的 parent key：指向精确 Binding
    --（room + access + 结构见证对）。
    UNIQUE (room_id, access_id, human_id, agent_id),
    -- 结构见证：Binding 必须在 COMMIT 期指向真实 direct pair
    --（Phase 2 事务先插 Binding，再建 direct pair；deferred 允许中间态）。
    FOREIGN KEY (room_id, human_id, agent_id)
        REFERENCES direct_room_pairs(room_id, human_id, agent_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (room_id <> '' AND instr(room_id, char(0)) = 0)
);

-- 插入资格（database 兜底，纵深防御）：Binding 只能在创建时插入，必须 access
-- active、target agent active 且 human/agent 与 access 一致、最新 Publication
-- service_status='accepting'；listing_visibility 此时不要求 invite_only（Owner
-- 下架后已有 Access 仍可新建 Room）。human 的 active 状态由 access 授予时的
-- 不变量与 participants 全局 CHECK 保证，此处以 defensively 再校验。
CREATE TRIGGER service_direct_room_bindings_insert_guard
BEFORE INSERT ON service_direct_room_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM agent_service_accesses sa
    WHERE sa.access_id = NEW.access_id
      AND sa.status = 'active'
      AND sa.agent_id = NEW.agent_id
      AND sa.human_id = NEW.human_id
      AND EXISTS (SELECT 1 FROM participants a
                  WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active')
      AND EXISTS (SELECT 1 FROM participants h
                  WHERE h.id = NEW.human_id AND h.kind = 'human' AND h.status = 'active')
      AND EXISTS (
          SELECT 1 FROM agent_service_publication_versions v
          WHERE v.agent_id = NEW.agent_id
            AND v.publication_version = (
                SELECT MAX(v2.publication_version)
                FROM agent_service_publication_versions v2
                WHERE v2.agent_id = NEW.agent_id)
            AND v.service_status = 'accepting'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'service direct room binding requires an active access to an active agent with an accepting publication');
END;

-- Binding 身份与结构见证一经创建不可改写（room/access/human/agent/created_at
-- 全部固定；Binding 不可迁移到另一份 Access、另一 pair 或另一时间）。
CREATE TRIGGER service_direct_room_bindings_identity_immutable
BEFORE UPDATE OF room_id, access_id, human_id, agent_id, created_at
ON service_direct_room_bindings
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.access_id IS NOT OLD.access_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'service direct room binding is immutable');
END;

CREATE TRIGGER service_direct_room_bindings_no_delete
BEFORE DELETE ON service_direct_room_bindings
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'service direct room bindings cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id) 或 UNIQUE 补充身份冲突时先隐式删除
-- 冲突行，可绕过 no-delete / immutable 守卫重建或改写绑定；本守卫在 INSERT
-- 阶段直接拒绝任一身份已占用的写入。
CREATE TRIGGER service_direct_room_bindings_insert_identity_guard
BEFORE INSERT ON service_direct_room_bindings
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM service_direct_room_bindings
    WHERE room_id = NEW.room_id
       OR (room_id = NEW.room_id AND access_id = NEW.access_id
           AND human_id = NEW.human_id AND agent_id = NEW.agent_id)
)
BEGIN
    SELECT RAISE(ABORT, 'service direct room binding identity already exists');
END;

-- ============================================================
-- 6. Service Direct Room 创建幂等记录
-- ============================================================

CREATE TABLE service_direct_room_create_idempotency (
    human_id   TEXT NOT NULL REFERENCES participants(id),
    key        TEXT NOT NULL,
    agent_id   TEXT NOT NULL REFERENCES participants(id),
    access_id  TEXT NOT NULL REFERENCES agent_service_accesses(access_id),
    room_id    TEXT NOT NULL REFERENCES rooms(id),
    name       TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    -- deferred 复合外键：指向精确 Binding（room + access + actor(自 PK 起) +
    -- agent）；同上，只证明引用一致性。
    FOREIGN KEY (room_id, access_id, human_id, agent_id)
        REFERENCES service_direct_room_bindings(room_id, access_id, human_id, agent_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (name IS NULL
        OR (typeof(name) = 'text' AND instr(name, char(0)) = 0
            AND name <> ''
            AND length(CAST(name AS BLOB)) <= 80)),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER service_direct_room_create_idempotency_actor_human
BEFORE INSERT ON service_direct_room_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.human_id AND p.kind = 'human' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'service direct room create idempotency actor must be an active human');
END;

-- 幂等记录必须见证请求的规范标题（含 null）：name 写入时必须等于 rooms.name
-- （room 与标题都不可变，写入时证明即终局证明；IS 保证 NULL 与 NULL 相等）。
CREATE TRIGGER service_direct_room_create_idempotency_witness_name
BEFORE INSERT ON service_direct_room_create_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.name IS NEW.name
)
BEGIN
    SELECT RAISE(ABORT, 'service direct room create idempotency name must witness the room name');
END;

CREATE TRIGGER service_direct_room_create_idempotency_immutable
BEFORE UPDATE ON service_direct_room_create_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'service direct room create idempotency records are immutable');
END;

CREATE TRIGGER service_direct_room_create_idempotency_no_delete
BEFORE DELETE ON service_direct_room_create_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'service direct room create idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫把同 key 重映射到另一 Room/Binding/标题。
CREATE TRIGGER service_direct_room_create_idempotency_insert_identity_guard
BEFORE INSERT ON service_direct_room_create_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM service_direct_room_create_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'service direct room create idempotency identity already exists');
END;

-- ============================================================
-- 7. 重建 direct_room_pairs 的 active owner 守卫（加入 Service binding 分支）
-- ============================================================
-- 既有迁移 004 的守卫要求插入 direct pair 时该 Human 必须是 agent 的 active
-- Owner。Service 路径（Phase 2）允许没有 Owner 关系的 accepted Contact 与本
-- Agent 建立 Service Direct Room，因此扩展为：active Agent Owner **或** 存在
-- 同 room+human+agent 的 Binding 且其当前 Service 权威在 pair 插入时点仍成立。
-- ServiceDirectRoomBinding 只是结构见证，不是永久授权快照：仅凭一条 Binding
-- 存在不足以放行——pair 插入必须再证一条精确关联链：
--   - binding 精确匹配 room+human+agent（同 room、同 human、同 agent）；
--   - binding 所引用的 access 与 human+agent 一致且 status='active'；
--   - target agent 仍是 active Agent（纵深防御，即使 pair 自身 legacy 分支
--     判定不同）；
--   - 该 Agent 最新 Publication 版本 service_status='accepting'。
-- 不要求 listing_visibility='invite_only'：published listing 已下架
-- （unpublished）而既有 active Access 仍存在时，仍允许为既有 Access 新建
-- Room。保守原则：**严禁**任何「任意 active Access 可直接绕过」或「只要曾
-- 存在 binding 即放行」的宽松逻辑。
DROP TRIGGER direct_room_pairs_insert_active_owner;

CREATE TRIGGER direct_room_pairs_insert_active_owner
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM agent_owners
                 WHERE agent_id = NEW.agent_id AND human_id = NEW.human_id AND status = 'active')
  AND NOT EXISTS (
      SELECT 1
      FROM service_direct_room_bindings b
      JOIN agent_service_accesses sa ON sa.access_id = b.access_id
      JOIN agent_service_publication_versions pv
           ON pv.agent_id = b.agent_id
          AND pv.publication_version = (
              SELECT MAX(v2.publication_version)
              FROM agent_service_publication_versions v2
              WHERE v2.agent_id = b.agent_id)
      WHERE b.room_id = NEW.room_id
        AND b.human_id = NEW.human_id
        AND b.agent_id = NEW.agent_id
        AND sa.human_id = NEW.human_id
        AND sa.agent_id = NEW.agent_id
        AND sa.status = 'active'
        AND EXISTS (SELECT 1 FROM participants a
                    WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active')
        AND pv.service_status = 'accepting'
  )
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires an active agent owner or a same-room service binding');
END;

-- ============================================================
-- 8. 重建 agent_events_insert_guard（加入 Service 分支）========
-- ============================================================
-- 既有迁移 007/011 的守卫对 message.created 的 direct 分支只要求配对 Human 消息
-- + active target。Service 纵深守卫（CORE_V1_AGENT_SERVICE.md §5.2）：只要该
-- direct Room 存在 service binding，则插入 message.created 事件时其 Access 必须
-- active 且最新 Publication 必须 accepting；legacy（无 binding）direct 分支与
-- group mention 分支、message.recalled 校正分支**逐字保留**。该守卫同时挡住
-- messages AFTER INSERT 自动生成路径——Service 无权的消息连 Message/sequence 都
-- 不会留下（整条 message INSERT 语句失败回滚）。
DROP TRIGGER agent_events_insert_guard;

CREATE TRIGGER agent_events_insert_guard
BEFORE INSERT ON agent_events
FOR EACH ROW
WHEN NOT (
    (NEW.kind = 'message.created'
     AND EXISTS (
         SELECT 1
         FROM messages m
         JOIN direct_room_pairs p ON p.room_id = m.room_id AND p.human_id = m.actor_id
         WHERE m.id = NEW.message_id
           AND p.agent_id = NEW.target_agent_id
           AND m.created_at = NEW.created_at
           AND EXISTS (SELECT 1 FROM participants a
                       WHERE a.id = p.agent_id AND a.kind = 'agent' AND a.status = 'active')
           AND EXISTS (SELECT 1 FROM room_memberships mm
                       WHERE mm.room_id = p.room_id
                         AND mm.participant_id = p.agent_id
                         AND mm.active_marker = 1)
           AND (
               -- legacy：Room 无 service binding → 既有行为不变
               NOT EXISTS (SELECT 1 FROM service_direct_room_bindings pb
                           WHERE pb.room_id = p.room_id)
               OR
               -- service：binding 的 access 仍 active + 最新 publication accepting
               EXISTS (
                   SELECT 1
                   FROM service_direct_room_bindings pb
                   JOIN agent_service_accesses sa ON sa.access_id = pb.access_id
                   JOIN agent_service_publication_versions pv
                        ON pv.agent_id = pb.agent_id
                       AND pv.publication_version = (
                           SELECT MAX(v2.publication_version)
                           FROM agent_service_publication_versions v2
                           WHERE v2.agent_id = pb.agent_id)
                   WHERE pb.room_id = p.room_id
                     AND pb.human_id = p.human_id
                     AND pb.agent_id = p.agent_id
                     AND sa.status = 'active'
                     AND pv.service_status = 'accepting'
               )
           )
     ))
    OR (NEW.kind = 'message.created'
        AND EXISTS (
            SELECT 1
            FROM mentions mnt
            JOIN messages m ON m.id = mnt.message_id
            WHERE mnt.message_id = NEW.message_id
              AND mnt.target_agent_id = NEW.target_agent_id
              AND m.created_at = NEW.created_at
              AND EXISTS (SELECT 1 FROM participants a
                          WHERE a.id = NEW.target_agent_id AND a.kind = 'agent' AND a.status = 'active')
              AND EXISTS (SELECT 1 FROM room_memberships mm
                          WHERE mm.room_id = m.room_id
                            AND mm.participant_id = NEW.target_agent_id
                            AND mm.active_marker = 1)
        ))
    OR (NEW.kind = 'message.recalled'
        AND EXISTS (
            SELECT 1 FROM message_recalls r
            WHERE r.message_id = NEW.message_id AND r.recalled_at = NEW.created_at
        )
        AND EXISTS (
            SELECT 1 FROM agent_events e
            WHERE e.message_id = NEW.message_id
              AND e.target_agent_id = NEW.target_agent_id
              AND e.kind = 'message.created'
        ))
)
BEGIN
    SELECT RAISE(ABORT, 'agent event requires a paired human message or a mention of an active target agent');
END;