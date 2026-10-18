-- =========================================================================
-- 批次二：一间房 = 一段关系（V1 + 前置一，2026-09-27 主人批准）
-- =========================================================================
--
-- 症状：三种房 = 三套规则——`human_direct` 见证一段 contact 周期（每段至多一间，
-- 已正确）；`direct`（私人）**什么都不见证**（同一对可开无限多间）；`direct`
-- （服务）**一个 access 可绑多间**；`group` 房本身就是关系（可多间，合理）。
-- 连带：`rooms.name` 被塞进幂等身份（`direct_room_idempotency.name` + 见证触发器
-- ⇒「同 key 不同标题 → 409」）——**房的身份成了它叫什么**。
--
-- 目标：让 `direct` 也**见证关系**（照 `human_direct` 已经用对的模式），一条规则
-- 取代三套；**标题退出身份**。
--
-- 迁移治理（`CORE_V1_IM_CONTRACT.md:262`）：「旧开发数据一律整体丢弃重建，不提供
-- 旧库升级桥、兼容双读或回填；验收只认全新数据库与重复启动」⇒ 本迁移是
-- **schema-only**：重建表**不搬行**。真家走"删 core.db 重新奠基"。
--
-- ⚠ `rooms.name` **这一列保留**：它是**群名**在用的共享列（`create_group` 写它、
-- `group_create_idempotency.name` 也用它做身份判据）。本迁移只**取消 direct 房的
-- 标题**，并用一道守卫把"direct 房没有标题"钉成事实（见文件末尾）。

-- ── 1. 拆依赖（先子后父）─────────────────────────────────────────────────
-- `direct_room_idempotency` 与 `service_direct_room_bindings` 都通过复合外键指向
-- `direct_room_pairs`，必须先拆；三张表各自的触发器随表删除，本迁移逐条重建。

DROP TRIGGER direct_room_idempotency_insert_name_witness;

DROP TABLE direct_room_idempotency;
DROP TABLE service_direct_room_bindings;
DROP TABLE direct_room_pairs;

-- ── 2. direct_room_pairs（重建：加"见证的关系"）────────────────────────────
-- `relation` 二选一：`private` = 见证一段 Owner 关系（`agent_owners` 那一行）；
-- `service` = 见证一段 Service Access（`agent_service_accesses` 那一行）。
-- 唯一性：私人房按 (human_id, agent_id) 唯一（每段 Owner 关系一间）；服务房按
-- service_access_id 唯一（每条 Access 一间）。⇒ **每段关系至多一间**。
-- `relation` 有缺省值 `'private'`：不写这一列的写入按"私人房"处理（direct 房绝大多数
-- 是私人房；两条创建路径都**显式**写它）。**这不削弱任何判据**——下面的见证守卫照样
-- 要求"私人房必须有一段 active 的 Owner 关系"，缺省猜错会被守卫当场拒绝，不会被默认
-- 成功吞掉。
CREATE TABLE direct_room_pairs (
    room_id     TEXT NOT NULL REFERENCES room_coverage(room_id),
    human_id    TEXT NOT NULL REFERENCES participants(id),
    agent_id    TEXT NOT NULL REFERENCES participants(id),
    human_membership_id TEXT NOT NULL,
    agent_membership_id TEXT NOT NULL,
    role_owner  TEXT NOT NULL DEFAULT 'owner' CHECK (role_owner = 'owner'),
    role_member TEXT NOT NULL DEFAULT 'member' CHECK (role_member = 'member'),
    relation    TEXT NOT NULL DEFAULT 'private'
        CHECK (relation IN ('private', 'service')),
    service_access_id TEXT REFERENCES agent_service_accesses(access_id),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id),
    UNIQUE (room_id, human_id, agent_id),
    CHECK (human_id <> agent_id),
    -- 见证两者恰一：私人房没有 access；服务房必须指出 access。
    CHECK ((relation = 'private' AND service_access_id IS NULL)
        OR (relation = 'service' AND service_access_id IS NOT NULL)),
    -- 每条 Service Access 至多一间房（SQLite 的 UNIQUE 视 NULL 互不相等 ⇒ 不影响私人房）。
    UNIQUE (service_access_id),
    FOREIGN KEY (room_id, human_membership_id, role_owner)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, agent_membership_id, role_member)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED
);

-- 每对 Human–Agent 至多一间**私人**房（= 每段 Owner 关系一间）。
CREATE UNIQUE INDEX direct_room_pairs_private_relation
    ON direct_room_pairs(human_id, agent_id) WHERE relation = 'private';

-- ── 3. direct_room_pairs 的 7 道守卫（逐条重建自 202608110004）─────────────

-- 覆盖结构：direct 配对只允许指向 kind=direct 的 Room。
CREATE TRIGGER direct_room_pairs_insert_kind_guard
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM room_coverage c
                 WHERE c.room_id = NEW.room_id AND c.kind = 'direct')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires a direct room coverage');
END;

-- 配对方向：human_id 必须指向 Human，agent_id 必须指向 Agent。
CREATE TRIGGER direct_room_pairs_insert_direction
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair must be human -> agent');
END;

-- 配对双方必须是 active Participant。
CREATE TRIGGER direct_room_pairs_insert_active_participants
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND status = 'active')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires active human and active agent');
END;

-- 【批次二改写，原名保留】房必须见证一段**当前有效**的关系：
-- · 私人房 ⇒ 该 (human, agent) 有 active 的 Agent Owner 关系；
-- · 服务房 ⇒ 同房 Binding 精确匹配（room + 所指 Access + human + agent），且该
--   Access 与 human/agent 一致、active，target agent 仍是 active Agent，最新
--   Publication 仍 accepting（不要求 invite_only：已下架而既有 active Access
--   仍在时，仍可为既有 Access 建 Room）。
--
-- ⚠ **本刀只改「房见证什么」，不改权威判据**：上面服务房那一串逐字来自被本刀
-- 取代的 202608210001 §7（原文：Binding「只是结构见证，不是永久授权快照」，
-- 「**严禁**任何『任意 active Access 可直接绕过』或『只要曾存在 binding 即放行』
-- 的宽松逻辑」）。新增的 `relation` / `service_access_id` 只是把「见证的是哪一
-- 段关系」写成事实，**不放宽**任何一条既有判据。
CREATE TRIGGER direct_room_pairs_insert_active_owner
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT (
    (NEW.relation = 'private'
     AND EXISTS (SELECT 1 FROM agent_owners
                 WHERE agent_id = NEW.agent_id AND human_id = NEW.human_id
                   AND status = 'active'))
    OR
    (NEW.relation = 'service'
     AND EXISTS (
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
           AND b.access_id = NEW.service_access_id
           AND b.human_id = NEW.human_id
           AND b.agent_id = NEW.agent_id
           AND sa.human_id = NEW.human_id
           AND sa.agent_id = NEW.agent_id
           AND sa.status = 'active'
           AND EXISTS (SELECT 1 FROM participants a
                       WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active')
           AND pv.service_status = 'accepting'
     ))
)
BEGIN
    SELECT RAISE(ABORT, 'direct room pair must witness an active relation (owner relation or service access)');
END;

-- 配对身份（Room、Human、Agent、奠基 Membership、见证的关系、创建时间）
-- 一经创建不可改写。
CREATE TRIGGER direct_room_pairs_identity_immutable
BEFORE UPDATE OF room_id, human_id, agent_id, human_membership_id,
    agent_membership_id, relation, service_access_id, created_at ON direct_room_pairs
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.human_membership_id IS NOT OLD.human_membership_id
  OR NEW.agent_membership_id IS NOT OLD.agent_membership_id
  OR NEW.relation IS NOT OLD.relation
  OR NEW.service_access_id IS NOT OLD.service_access_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'direct room pair is immutable');
END;

CREATE TRIGGER direct_room_pairs_no_delete
BEFORE DELETE ON direct_room_pairs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'direct room pairs cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id) 冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫；本守卫在 INSERT 阶段直接拒绝 room_id 已占用的
-- 写入。「每段关系至多一间」由上面两条唯一约束（私人：部分唯一索引；服务：
-- UNIQUE(service_access_id)）承担。
CREATE TRIGGER direct_room_pairs_insert_identity_guard
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_pairs
    WHERE room_id = NEW.room_id
)
BEGIN
    SELECT RAISE(ABORT, 'direct room pair identity already exists');
END;

-- ── 4. service_direct_room_bindings（重建：同一 access 一间房）─────────────
CREATE TABLE service_direct_room_bindings (
    room_id    TEXT PRIMARY KEY,
    access_id  TEXT NOT NULL REFERENCES agent_service_accesses(access_id),
    human_id   TEXT NOT NULL REFERENCES participants(id),
    agent_id   TEXT NOT NULL REFERENCES participants(id),
    created_at INTEGER NOT NULL,
    CHECK (human_id <> agent_id),
    -- 【批次二】同一 Access 至多一间服务房（原来只有 room_id 唯一 ⇒ 可绑多间）。
    UNIQUE (access_id),
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
-- service_status='accepting'；listing_visibility 此时不要求 invite_only。
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

-- Binding 身份与结构见证一经创建不可改写。
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
-- 冲突行；本守卫在 INSERT 阶段直接拒绝任一身份已占用的写入。
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

-- ── 5. direct_room_idempotency（重建：去掉 name）──────────────────────────
-- 标题不再进幂等身份 ⇒「同 key 不同标题 → 409」随之取消（那一格是"房的身份
-- 是它叫什么"的产物）。整行仍不可改写、不可删除。
CREATE TABLE direct_room_idempotency (
    human_id       TEXT NOT NULL REFERENCES participants(id),
    key            TEXT NOT NULL,
    agent_id       TEXT NOT NULL REFERENCES participants(id),
    room_id        TEXT NOT NULL REFERENCES rooms(id),
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    FOREIGN KEY (room_id, human_id, agent_id)
        REFERENCES direct_room_pairs(room_id, human_id, agent_id),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

CREATE TRIGGER direct_room_idempotency_immutable
BEFORE UPDATE OF human_id, key, agent_id, room_id, created_at
ON direct_room_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency records are immutable');
END;

CREATE TRIGGER direct_room_idempotency_no_delete
BEFORE DELETE ON direct_room_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency records cannot be deleted');
END;

CREATE TRIGGER direct_room_idempotency_insert_identity_guard
BEFORE INSERT ON direct_room_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency identity already exists');
END;

-- ── 6. direct 房没有标题（把裁决钉成事实，不只靠约定）─────────────────────
-- `rooms.name` 保留（群名在用它），但 direct 房一律为 NULL。

CREATE TRIGGER rooms_direct_rooms_have_no_title_insert
BEFORE INSERT ON rooms
FOR EACH ROW
WHEN NEW.kind = 'direct' AND NEW.name IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'direct rooms have no title');
END;

CREATE TRIGGER rooms_direct_rooms_have_no_title_update
BEFORE UPDATE OF name, kind ON rooms
FOR EACH ROW
WHEN NEW.kind = 'direct' AND NEW.name IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'direct rooms have no title');
END;
