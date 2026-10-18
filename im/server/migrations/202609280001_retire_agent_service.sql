-- =========================================================================
-- 服务层退役（Publication / Access / 服务房）—— 2026-09-28 主人裁决
-- =========================================================================
--
-- 授权：「先做基础 IM 那种，不弄那么麻烦，就是比如什么市场什么的……就传统 IM 那种」
-- （2026-09-27）+「行可以做减法吧」（2026-09-28）。
--
-- **为什么退役（一句原理）**：服务声明层是**工作态**（"我能提供什么服务"），不是
-- IM 事实。传统 IM 里没有这一层；它的存在违反了 2026-09-26 那条红线——**执行过程
-- 不经过 IM**（能提供什么服务属于执行端那一层，与"在不在家"同类）。同时它把
-- **预先同意**（grant access before asking）引入了一个只有事后同意（accept /
-- decline）的模型，多出一整套平行语义。**双平面下 IM 就是 IM**：把 im 服务端换成
-- 微信/QQ 仍然成立的那一面才留下。
--
-- 本迁移做四件事（**schema-only；唯一的行搬运是既有 direct 房与它的幂等记录**）：
--   1. 重建 `direct_room_pairs`：**去掉 `relation` / `service_access_id` 两列**
--      ——`direct` 房只剩一种来路（一段 Agent Owner 关系），不再需要"是哪段关系"
--      的判别列；"每段关系至多一间"由**普通 `UNIQUE(human_id, agent_id)`** 承担
--      （原为 `WHERE relation='private'` 的部分唯一索引，语义一字不变）；
--   2. 重建 `direct_room_idempotency`（它的复合外键指向 1 里那张表）；
--   3. 重建两道**内嵌了服务分支**的既有守卫：
--      `direct_room_pairs_insert_active_owner`（去掉 service 分支）与
--      `agent_events_insert_guard`（回到 202608130001 的 legacy 形状）；
--   4. 拆掉一条**挂在 `participants` 上**的服务触发器
--      （`agent_service_accesses_agent_revoke_propagates`：表删了它不会跟着删，
--      留着会让每一次 Agent 撤销 500），并 `DROP TABLE` 六张服务表
--      （表上的触发器与索引随表消失）。
--
-- ⚠ **为什么必须重建 direct_room_pairs，而不是只 DROP 六张表**（施工中核出）：
-- `direct_room_pairs.service_access_id` 上有 `REFERENCES agent_service_accesses(access_id)`。
-- SQLite 不支持 ALTER TABLE DROP CONSTRAINT；**父表被 DROP 之后，子表上那条外键
-- 会让任何 INSERT 当场失败**（实测：`no such table: main.agent_service_accesses`，
-- 即使子键为 NULL）⇒ 只删服务表会把"建私人房"整条路打断。同理，
-- `direct_room_pairs_insert_active_owner` 与 `agent_events_insert_guard` 的正文里
-- 直接 SELECT 服务表，触发器正文在 DML 时才解析表引用 ⇒ 不重建就会在
-- **建房**与**Human→Agent 消息生成 Event** 两处炸掉。
--
-- ⚠ **本刀不收回批次二**：「一间房 = 一段关系」这条裁决一字不动（它管的是
-- **基数**：每段关系至多一间）。退役掉的只是"direct 房有两种关系"这一支
-- ——那一种（服务房）本身没了。
--
-- 迁移治理（`CORE_V1_IM_CONTRACT.md`）：不提供旧库升级桥、不双读、不回填；
-- 但**既有公共事实一行不丢**（真家已有 direct 房与消息）⇒ 本迁移**原地重建 +
-- 搬运既有行**，不需要删库重新奠基。
--
-- 前置（2026-09-28 真家实测，只读）：六张服务表合计 **0 行**
-- （`agent_service_publication_versions` / `..._put_idempotency` /
-- `agent_service_accesses` / `..._grant_idempotency` /
-- `service_direct_room_bindings` / `service_direct_room_create_idempotency`）。
-- 下面的 `retire_guard` 把这条前提**钉成断言**：真家若出现服务房，本迁移当场
-- 失败，绝不静默丢弃事实。

-- ── 1. 拆子表依赖（先子后父）─────────────────────────────────────────────
-- `direct_room_idempotency` 与 `direct_room_pairs` 的行都必须原样保留 ⇒ 先搬进
-- 影子表，再整表退场（触发器随表删除），最后以**同一个名字**重建。
--
-- ⚠ **为什么是"先 DROP 再 CREATE"而不是 `ALTER TABLE … RENAME`**（施工中实测）：
-- 别的表上还有触发器正文引用 `direct_room_pairs`（如
-- `messages_after_insert_generates_agent_event`），而 SQLite 的 RENAME 会
-- **重新解析所有触发器正文**——旧表已经没了，于是当场报
-- `error in trigger …: no such table: main.direct_room_pairs`。
-- 先 DROP 再以原名重建则完全不碰那些触发器（它们只在 DML 时才解析表引用）。
-- `service_direct_room_bindings` 同样是子表，但它是**服务事实**：退役，不搬。

CREATE TABLE direct_room_idempotency_carry AS SELECT * FROM direct_room_idempotency;
CREATE TABLE direct_room_pairs_carry AS SELECT * FROM direct_room_pairs;

DROP TABLE direct_room_idempotency;
DROP TABLE service_direct_room_bindings;
DROP TABLE direct_room_pairs;

-- 前置断言（fail-closed）：服务房必须一行都没有。
CREATE TEMP TABLE retire_agent_service_guard (x INTEGER CHECK (x = 0));
INSERT INTO retire_agent_service_guard
    SELECT COUNT(*) FROM direct_room_pairs_carry WHERE relation = 'service';
DROP TABLE retire_agent_service_guard;

-- ── 2. direct_room_pairs（以原名重建：direct 房只剩一段关系）───────────────
-- 七道守卫随旧表删除，本迁移在 §4 逐条重建（正文只去掉 relation 判别）。

CREATE TABLE direct_room_pairs (
    room_id     TEXT NOT NULL REFERENCES room_coverage(room_id),
    human_id    TEXT NOT NULL REFERENCES participants(id),
    agent_id    TEXT NOT NULL REFERENCES participants(id),
    human_membership_id TEXT NOT NULL,
    agent_membership_id TEXT NOT NULL,
    role_owner  TEXT NOT NULL DEFAULT 'owner' CHECK (role_owner = 'owner'),
    role_member TEXT NOT NULL DEFAULT 'member' CHECK (role_member = 'member'),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id),
    -- 复合外键的 parent key（`direct_room_idempotency` 指向这三列）。
    UNIQUE (room_id, human_id, agent_id),
    CHECK (human_id <> agent_id),
    -- **每段关系至多一间**（批次二的原判据；原为部分唯一索引
    -- `WHERE relation = 'private'`，如今 private 是唯一取值）。
    UNIQUE (human_id, agent_id),
    FOREIGN KEY (room_id, human_membership_id, role_owner)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, agent_membership_id, role_member)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED
);

-- 既有行原样搬回（守卫在 §4 才重建：这些行是**已经发生过的事实**，不是新写入
-- ——与"守卫管 INSERT"同一口径）。
INSERT INTO direct_room_pairs
    (room_id, human_id, agent_id, human_membership_id, agent_membership_id,
     role_owner, role_member, created_at)
SELECT room_id, human_id, agent_id, human_membership_id, agent_membership_id,
       role_owner, role_member, created_at
FROM direct_room_pairs_carry;

DROP TABLE direct_room_pairs_carry;

-- ── 3. direct_room_idempotency（重建：外键指向新表）───────────────────────
-- 形状与 202609270004 §5 逐字相同（`name` 那一列在批次二就已退出身份）。

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

INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at)
SELECT human_id, key, agent_id, room_id, created_at FROM direct_room_idempotency_carry;

DROP TABLE direct_room_idempotency_carry;

-- ── 4. direct_room_pairs 的 7 道守卫（逐条重建）───────────────────────────

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

-- 房必须见证一段**当前有效**的关系：该 (human, agent) 有 active 的 Agent Owner
-- 关系，**或**一段 accepted 关系段（批次三第三刀 第③步：准入 = ownership ∪
-- accepted 关系，与群邀请资格同形）。service 分支已随服务层退役删除。
CREATE TRIGGER direct_room_pairs_insert_active_owner
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT (
    EXISTS (SELECT 1 FROM agent_owners
            WHERE agent_id = NEW.agent_id AND human_id = NEW.human_id
              AND status = 'active')
    OR
    EXISTS (SELECT 1 FROM contact_periods c
            WHERE c.active_marker = 1 AND c.state = 'accepted'
              AND ((c.a_id = NEW.human_id AND c.b_id = NEW.agent_id)
                OR (c.a_id = NEW.agent_id AND c.b_id = NEW.human_id)))
)
BEGIN
    SELECT RAISE(ABORT, 'direct room pair must witness an active owner or an accepted contact');
END;

-- 配对身份（Room、Human、Agent、奠基 Membership、创建时间）一经创建不可改写。
CREATE TRIGGER direct_room_pairs_identity_immutable
BEFORE UPDATE OF room_id, human_id, agent_id, human_membership_id,
    agent_membership_id, created_at ON direct_room_pairs
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.human_membership_id IS NOT OLD.human_membership_id
  OR NEW.agent_membership_id IS NOT OLD.agent_membership_id
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
-- 写入。「每段关系至多一间」由上面的 `UNIQUE(human_id, agent_id)` 承担。
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

-- ── 5. direct_room_idempotency 的 3 道守卫（逐条重建）─────────────────────

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

-- ── 6. agent_events_insert_guard（重建：回到 legacy 形状）─────────────────
-- 正文逐字取自 202608130001（服务分支加入之前的那一版）：direct 分支只要求
-- 「配对的 Human 消息 + active target + active membership」，不再有"该房若存在
-- service binding 则还要 Access active + Publication accepting"那一段。
-- mention 分支与 message.recalled 分支**一字不动**。

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

-- ── 7. 六张服务表退场（触发器与索引随表消失）──────────────────────────────
--
-- ⚠ **先拆一条挂在 participants 上的触发器**（施工中实测踩到）：表没了，触发器
-- 不一定跟着没——`agent_service_accesses_agent_revoke_propagates` 是**挂在
-- `participants` 上的**（Agent 撤销时把它的 Access 一并置 revoked），正文里
-- `UPDATE agent_service_accesses`。它**不随服务表删除**，于是之后每一次
-- Agent 撤销（`DELETE /v1/agents/{id}` 与级联）都会在 DML 时炸
-- `no such table: agent_service_accesses` ⇒ 真机表现为 **500 internal_error**。
-- 这一条必须显式 DROP。

DROP TRIGGER agent_service_accesses_agent_revoke_propagates;

DROP TABLE agent_service_publication_put_idempotency;
DROP TABLE agent_service_access_grant_idempotency;
DROP TABLE service_direct_room_create_idempotency;
DROP TABLE agent_service_publication_versions;
DROP TABLE agent_service_accesses;
