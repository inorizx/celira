-- Celira Core V1：关系可逆性（拒绝 / 解除 / 拉黑）切片。
--
-- 契约：CORE_V1_CONTACT_REVERSIBILITY.md（唯一权威契约）
--
-- 关系从「一锤定音的行」升级为「可结束的周期」：结束只追加新事实、绝不
-- 改写历史；发言准入仍只由房间成员资格一个裁决者决定；黑名单只管新关系的
-- 入口，不进消息主链；拉黑 ⊇ 解除（拉黑必先结束既有关系）。
--
-- 1. contacts → contact_periods（周期表）：
--    - period_id（UUID v4）主键；规范 (a_id, b_id) 序（a_id < b_id）；
--      requester/addressee 方向；state ∈ ('pending','accepted')；
--      accepted_at、closed_at、closed_by ∈ ('declined','ended')、active_marker；
--    - 「每对至多一个 active 段」由部分唯一索引 ux_contact_periods_single_active
--      表达（闭合段不占位，可累积多条：申请→拒绝→再申请→接受→解除→再申请
--      都是独立周期）；
--    - 闭合段整行不可改写且不可删；既有守卫逐条平移（两端 active Human、
--      方向 CHECK、身份不可改写、INSERT 身份防改写 / REPLACE 防线、
--      UNIQUE(requester_id, idempotency_key) 保留——幂等键按 requester
--      跨全部周期唯一，闭合段仍占位）；
--    - 状态机：pending → accepted 是唯一 state 迁移（必须同时写 accepted_at）；
--      拒绝与解除是「关闭周期」（active_marker 1→0 + closed_at + closed_by），
--      不改写 state：declined 只能关闭 pending 段，ended 关闭 pending 或
--      accepted 段；闭合后整行不可变。
--
-- 2. human_direct_pairs 重建：唯一性从「一对人」改为「一段关系」——
--    UNIQUE(contact_period_id)，以 (contact_period_id, human_a_id, human_b_id)
--    复合外键见证奠基段（contact_periods 上建 UNIQUE(period_id, a_id, b_id)
--    parent key）；建房守卫改为「所引用的段当前 active 且 accepted」。段与
--    房间 1:1：同一对 Human 在下一次关系周期可以另建新房，与已批准的
--    Direct Room 多房间解冻同构。human_direct_room_idempotency 的复合外键
--    目标列不变（仍是 (room_id, human_a_id, human_b_id)）。
--
-- 3. room_memberships 重建（表级 CHECK 无法原位改）：closed_by 词表加
--    contact_ended；全部既有守卫、部分唯一索引、被其它表当作 deferred 外键
--    parent key 的 UNIQUE 约束逐条平移，不顺手简化；被其它表触发器按名字
--    引用它的地方（agent_revoke_propagates_room_memberships、human event
--    系列、agent_events 系列、read_cursors_membership_insert）重建后仍可解析
--    （ON room_memberships 的触发器随表删除，本迁移逐条重建）。
--
-- 4. contact_periods_end_propagates_room_memberships：段以 ended 关闭的同一
--    事务内关闭双方在该 human_direct 房间的 active 成员周期
--    （left_before_sequence = 该房当前最大 message sequence，
--    closed_by='contact_ended'，left_at = 段的 closed_at）。形状照抄
--    agent_revoke_propagates_room_memberships。
--
-- 5. room_memberships_human_close_guard 放宽：human_direct 房间的 Human 周期
--    允许以 contact_ended 关闭，但必须同事务有精确见证——该房
--    human_direct_pairs.contact_period_id 指向的段已 active_marker=0 且
--    closed_at = NEW.left_at（照抄 transfer 的同事务精确见证纪律）。
--
-- 6. participant_blocks：block_id 主键、blocker_id/blocked_id（两端必须
--    active Human，禁止自拉黑）、created_at、unblocked_at、active_marker；
--    部分唯一「每对每方向至多一个 active 段」；不可删、身份不可改写、闭合
--    终态、REPLACE 防线。
--
-- 7. human_event_facts_insert_guard 的 member.left 白名单加 contact_ended
--    （不新增事件 kind）；段结束关闭双方成员周期后，在
--    contact_periods_end_propagates_room_memberships 内为双方各生成一条
--    member.left 事实（此时房内已无 active Human 成员，扇出只产生主体本人
--    的自投递，对端不被打扰）；human_event_facts_member_left 触发器明确
--    排除 contact_ended（避免中途半关闭状态产生跨方投递）。
--
-- 8. agent_service_accesses_insert_guard 重建：accepted Contact 资格改读
--    contact_periods（state='accepted' AND active_marker=1）。
--
-- 表重建沿用项目破坏式收敛先例（202608110003）：PRAGMA defer_foreign_keys
-- 让外键检查推迟到 COMMIT，从而可在同一 migration 事务内安全 DROP / RENAME；
-- 既有 contacts / human_direct_pairs / room_memberships 数据原样迁移（不丢
-- 已有关系、配对与成员周期）。PRAGMA legacy_alter_table 让 RENAME 不做
-- 「重写其它对象引用」的扫描——本迁移只把 *_new 重命名回它们原本的名字，
-- 其它对象引用的正是这个名字，无需也绝不能改写（否则 SQLite 会在扫描时
-- 因旧表已 DROP 而报 `no such table`）。

PRAGMA defer_foreign_keys = ON;
PRAGMA legacy_alter_table = ON;

-- =========================================================================
-- 1. contact_periods
-- =========================================================================

CREATE TABLE contact_periods (
    period_id       TEXT PRIMARY KEY,
    a_id            TEXT NOT NULL REFERENCES participants(id),
    b_id            TEXT NOT NULL REFERENCES participants(id),
    requester_id    TEXT NOT NULL REFERENCES participants(id),
    addressee_id    TEXT NOT NULL REFERENCES participants(id),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'accepted')),
    created_at      INTEGER NOT NULL,
    accepted_at     INTEGER,
    closed_at       INTEGER,
    closed_by       TEXT CHECK (closed_by IN ('declined', 'ended')),
    active_marker   INTEGER NOT NULL DEFAULT 1 CHECK (active_marker IN (0, 1)),
    idempotency_key TEXT NOT NULL,
    CHECK (a_id < b_id),
    CHECK ((requester_id = a_id AND addressee_id = b_id)
        OR (requester_id = b_id AND addressee_id = a_id)),
    CHECK (requester_id <> addressee_id),
    CHECK ((state = 'accepted' AND accepted_at IS NOT NULL)
        OR (state = 'pending' AND accepted_at IS NULL)),
    -- active_marker 与闭合二元组保持一致：active 段 closed 字段全 NULL；
    -- 闭合段 closed_at / closed_by 全非 NULL。
    CHECK ((active_marker = 1 AND closed_at IS NULL AND closed_by IS NULL)
        OR (active_marker = 0 AND closed_at IS NOT NULL AND closed_by IS NOT NULL)),
    CHECK (typeof(period_id) = 'text'),
    CHECK (length(period_id) = 36),
    CHECK (substr(period_id, 9, 1) = '-'),
    CHECK (substr(period_id, 14, 1) = '-'),
    CHECK (substr(period_id, 19, 1) = '-'),
    CHECK (substr(period_id, 24, 1) = '-'),
    CHECK (substr(period_id, 15, 1) = '4'),
    CHECK (substr(period_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (period_id = lower(period_id)),
    CHECK (instr(period_id, char(0)) = 0),
    CHECK (period_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(period_id, '-', '')) = 32),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*'),
    -- human_direct_pairs 复合外键的 parent key：奠基段以 (period_id, a_id,
    -- b_id) 精确见证（段与配对两端 Human 一一对应）。
    UNIQUE (period_id, a_id, b_id),
    -- 请求幂等键按 requester 唯一（跨全部周期，含闭合段；重新申请必须新 key）。
    UNIQUE (requester_id, idempotency_key)
);

CREATE INDEX idx_contact_periods_a ON contact_periods(a_id);
CREATE INDEX idx_contact_periods_b ON contact_periods(b_id);
CREATE INDEX idx_contact_periods_addressee ON contact_periods(addressee_id);

-- 同对至多一个 active 段（部分唯一索引，仅约束 active_marker=1；闭合段不
-- 占位，可累积多条）。REPLACE 的隐式删除发生在 BEFORE INSERT 触发器之后，
-- 身份守卫在第一次触发时即看到既有 active 段并拒绝改写。
CREATE UNIQUE INDEX ux_contact_periods_single_active
    ON contact_periods(a_id, b_id)
    WHERE active_marker = 1;

-- 新段：两端必须是不同且 active 的 Human（词表/方向/状态由 CHECK 兜底）。
CREATE TRIGGER contact_periods_insert_guard
BEFORE INSERT ON contact_periods
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p1 WHERE p1.id = NEW.a_id AND p1.kind = 'human' AND p1.status = 'active'
) OR NOT EXISTS (
    SELECT 1 FROM participants p2 WHERE p2.id = NEW.b_id AND p2.kind = 'human' AND p2.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'contact period requires two different active humans');
END;

-- 任何 INSERT 只能创建 active 段（closed 只能由 UPDATE 关闭路径产生，不能
-- 直接插入，也不能经 REPLACE 复活）。
CREATE TRIGGER contact_periods_insert_active_only
BEFORE INSERT ON contact_periods
FOR EACH ROW
WHEN NEW.active_marker <> 1 OR NEW.closed_at IS NOT NULL OR NEW.closed_by IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new contact periods must be active');
END;

-- 状态机：pending → accepted 是唯一 state 迁移（必须同时写入 accepted_at）；
-- accepted_at 一经写入不可改写（等值自赋值不触发）。
CREATE TRIGGER contact_periods_state_machine
BEFORE UPDATE OF state, accepted_at ON contact_periods
FOR EACH ROW
WHEN (NEW.state IS NOT OLD.state OR NEW.accepted_at IS NOT OLD.accepted_at)
  AND NOT (OLD.state = 'pending' AND NEW.state = 'accepted'
           AND OLD.accepted_at IS NULL AND NEW.accepted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'contact period state can only move once from pending to accepted');
END;

-- 段身份（period_id、两端、方向、幂等键、创建时间）不可改写。
CREATE TRIGGER contact_periods_identity_immutable
BEFORE UPDATE OF period_id, a_id, b_id, requester_id, addressee_id, idempotency_key, created_at
ON contact_periods
FOR EACH ROW
WHEN NEW.period_id IS NOT OLD.period_id
  OR NEW.a_id IS NOT OLD.a_id
  OR NEW.b_id IS NOT OLD.b_id
  OR NEW.requester_id IS NOT OLD.requester_id
  OR NEW.addressee_id IS NOT OLD.addressee_id
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'contact period identity is immutable');
END;

-- 关闭段必须一次写满闭合二元组且 closed_by 合法；active_marker 必须 1→0。
CREATE TRIGGER contact_periods_close_requires_full_tuple
BEFORE UPDATE OF active_marker, closed_at, closed_by ON contact_periods
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND (NEW.closed_at IS NULL OR NEW.closed_by IS NULL
       OR NEW.closed_by NOT IN ('declined', 'ended'))
BEGIN
    SELECT RAISE(ABORT, 'closing a contact period requires closed_at and a valid closed_by');
END;

-- 关闭原因与段状态耦合：declined 只能关闭 pending 段（拒绝未接受的申请）；
-- ended 关闭 pending 或 accepted 段（任一方单方面解除 / 拉黑结束既有关系）。
CREATE TRIGGER contact_periods_close_reason_matches_state
BEFORE UPDATE OF active_marker, closed_at, closed_by ON contact_periods
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND NOT (NEW.closed_by = 'ended'
           OR (NEW.closed_by = 'declined' AND OLD.state = 'pending'))
BEGIN
    SELECT RAISE(ABORT, 'declined closes a pending period and ended closes an active period');
END;

-- 闭合段是终态：整行不可再改写（含复活为 active 与改写任何字段）。
CREATE TRIGGER contact_periods_closed_immutable
BEFORE UPDATE ON contact_periods
FOR EACH ROW
WHEN OLD.active_marker = 0
  AND (NEW.period_id IS NOT OLD.period_id
       OR NEW.a_id IS NOT OLD.a_id
       OR NEW.b_id IS NOT OLD.b_id
       OR NEW.requester_id IS NOT OLD.requester_id
       OR NEW.addressee_id IS NOT OLD.addressee_id
       OR NEW.state IS NOT OLD.state
       OR NEW.created_at IS NOT OLD.created_at
       OR NEW.accepted_at IS NOT OLD.accepted_at
       OR NEW.closed_at IS NOT OLD.closed_at
       OR NEW.closed_by IS NOT OLD.closed_by
       OR NEW.active_marker IS NOT OLD.active_marker
       OR NEW.idempotency_key IS NOT OLD.idempotency_key)
BEGIN
    SELECT RAISE(ABORT, 'closed contact periods are immutable');
END;

-- 段不允许硬删除。
CREATE TRIGGER contact_periods_no_delete
BEFORE DELETE ON contact_periods
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'contact periods cannot be deleted');
END;

-- 身份防改写：REPLACE / INSERT OR REPLACE 在 period_id、(requester_id,
-- idempotency_key) 或「同对 active 段」冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫改写或重建既有事实；本守卫在 INSERT 阶段直接
-- 拒绝任一身份已占用的写入。
CREATE TRIGGER contact_periods_insert_identity_guard
BEFORE INSERT ON contact_periods
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM contact_periods
    WHERE period_id = NEW.period_id
       OR (requester_id = NEW.requester_id AND idempotency_key = NEW.idempotency_key)
       OR (a_id = NEW.a_id AND b_id = NEW.b_id
           AND NEW.active_marker = 1 AND active_marker = 1)
)
BEGIN
    SELECT RAISE(ABORT, 'contact period identity already exists');
END;

-- 既有 contacts 数据原样迁移为 active 段（period_id 由 SQLite 生成 UUID v4）。
INSERT INTO contact_periods
    (period_id, a_id, b_id, requester_id, addressee_id, state, created_at,
     accepted_at, closed_at, closed_by, active_marker, idempotency_key)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
    '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
    substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
    lower(hex(randomblob(6))),
    a_id, b_id, requester_id, addressee_id, status, created_at,
    accepted_at, NULL, NULL, 1, idempotency_key
FROM contacts;

-- =========================================================================
-- 2. room_memberships 重建（closed_by 词表加 contact_ended）
-- =========================================================================

CREATE TABLE room_memberships_new (
    membership_id        TEXT PRIMARY KEY,
    room_id              TEXT NOT NULL REFERENCES rooms(id),
    participant_id       TEXT NOT NULL REFERENCES participants(id),
    role                 TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    joined_at            INTEGER NOT NULL,
    join_after_sequence  INTEGER NOT NULL CHECK (join_after_sequence >= 0),
    left_at              INTEGER,
    left_before_sequence INTEGER,
    closed_by            TEXT,
    active_marker        INTEGER NOT NULL DEFAULT 1 CHECK (active_marker IN (0, 1)),
    CHECK (typeof(membership_id) = 'text'),
    CHECK (length(membership_id) = 36),
    CHECK (substr(membership_id, 9, 1) = '-'),
    CHECK (substr(membership_id, 14, 1) = '-'),
    CHECK (substr(membership_id, 19, 1) = '-'),
    CHECK (substr(membership_id, 24, 1) = '-'),
    CHECK (substr(membership_id, 15, 1) = '4'),
    CHECK (substr(membership_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (membership_id = lower(membership_id)),
    CHECK (instr(membership_id, char(0)) = 0),
    CHECK (membership_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(membership_id, '-', '')) = 32),
    CHECK ((active_marker = 1 AND left_at IS NULL AND left_before_sequence IS NULL AND closed_by IS NULL)
        OR (active_marker = 0 AND left_at IS NOT NULL AND left_before_sequence IS NOT NULL
            AND closed_by IS NOT NULL
            AND closed_by IN ('leave', 'removed', 'transfer', 'agent_revoked', 'contact_ended'))),
    CHECK (left_before_sequence IS NULL OR left_before_sequence >= join_after_sequence),
    UNIQUE (room_id, membership_id, role),
    UNIQUE (room_id, membership_id, role, active_marker),
    UNIQUE (room_id, membership_id, participant_id, role, left_at,
        left_before_sequence, closed_by),
    UNIQUE (room_id, membership_id, participant_id, role, joined_at,
        join_after_sequence),
    UNIQUE (room_id, participant_id, membership_id)
);

INSERT INTO room_memberships_new
    SELECT membership_id, room_id, participant_id, role, joined_at,
           join_after_sequence, left_at, left_before_sequence, closed_by, active_marker
    FROM room_memberships;

DROP TABLE room_memberships;

ALTER TABLE room_memberships_new RENAME TO room_memberships;

-- =========================================================================
-- 3. human_direct_pairs 重建（唯一性从「一对人」改为「一段关系」）
-- =========================================================================

CREATE TABLE human_direct_pairs_new (
    room_id     TEXT NOT NULL REFERENCES room_coverage(room_id),
    human_a_id  TEXT NOT NULL REFERENCES participants(id),
    human_b_id  TEXT NOT NULL REFERENCES participants(id),
    contact_period_id TEXT NOT NULL,
    human_a_membership_id TEXT NOT NULL,
    human_b_membership_id TEXT NOT NULL,
    role_a      TEXT NOT NULL CHECK (role_a IN ('owner', 'member')),
    role_b      TEXT NOT NULL CHECK (role_b IN ('owner', 'member')),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id),
    -- 段与房间 1:1：一段关系至多一个 human_direct Room；下一段关系可另建新房。
    UNIQUE (contact_period_id),
    -- room_coverage 与 human_direct_room_idempotency 复合外键的 parent key
    -- （目标列不变）。
    UNIQUE (room_id, human_a_id, human_b_id),
    CHECK (human_a_id < human_b_id),
    CHECK (role_a <> role_b),
    CHECK ((role_a = 'owner' AND role_b = 'member')
        OR (role_a = 'member' AND role_b = 'owner')),
    -- 奠基段见证：复合外键强制 human_a/human_b 与段的 a_id/b_id 精确一致。
    FOREIGN KEY (contact_period_id, human_a_id, human_b_id)
        REFERENCES contact_periods(period_id, a_id, b_id),
    FOREIGN KEY (room_id, human_a_membership_id, role_a)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, human_b_membership_id, role_b)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED
);

INSERT INTO human_direct_pairs_new
    (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id,
     human_b_membership_id, role_a, role_b, created_at)
SELECT hp.room_id, hp.human_a_id, hp.human_b_id, cp.period_id,
       hp.human_a_membership_id, hp.human_b_membership_id, hp.role_a, hp.role_b,
       hp.created_at
FROM human_direct_pairs hp
JOIN contact_periods cp ON cp.a_id = hp.human_a_id AND cp.b_id = hp.human_b_id;

DROP TABLE human_direct_pairs;

ALTER TABLE human_direct_pairs_new RENAME TO human_direct_pairs;

-- contacts 不再被任何事实引用（human_direct_pairs 已改为见证段）。
DROP TABLE contacts;

-- =========================================================================
-- 4. room_memberships 索引与触发器（逐条平移；closed_by 白名单加 contact_ended）
-- =========================================================================

CREATE UNIQUE INDEX ux_room_memberships_single_active_period
    ON room_memberships(room_id, participant_id)
    WHERE active_marker = 1;

CREATE UNIQUE INDEX ux_room_memberships_single_active_owner
    ON room_memberships(room_id)
    WHERE role = 'owner' AND active_marker = 1;

CREATE INDEX idx_room_memberships_participant ON room_memberships(participant_id);
CREATE INDEX idx_room_memberships_room_active ON room_memberships(room_id, active_marker);

CREATE TRIGGER room_memberships_insert_structure_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.kind = 'group'
)
AND NOT EXISTS (
    SELECT 1 FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id
      AND ((NEW.membership_id = p.human_membership_id AND NEW.participant_id = p.human_id
            AND NEW.role = 'owner')
        OR (NEW.membership_id = p.agent_membership_id AND NEW.participant_id = p.agent_id
            AND NEW.role = 'member'))
)
AND NOT EXISTS (
    SELECT 1 FROM human_direct_pairs h
    WHERE h.room_id = NEW.room_id
      AND ((NEW.membership_id = h.human_a_membership_id AND NEW.participant_id = h.human_a_id
            AND NEW.role = h.role_a)
        OR (NEW.membership_id = h.human_b_membership_id AND NEW.participant_id = h.human_b_id
            AND NEW.role = h.role_b))
)
BEGIN
    SELECT RAISE(ABORT, 'membership does not match the room structure');
END;

CREATE TRIGGER room_memberships_active_requires_active_participant_insert
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker = 1
  AND NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'active membership requires active participant');
END;

CREATE TRIGGER room_memberships_insert_active_only
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker <> 1
  OR NEW.left_at IS NOT NULL
  OR NEW.left_before_sequence IS NOT NULL
  OR NEW.closed_by IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new room memberships must be active periods with a null left tuple');
END;

CREATE TRIGGER room_memberships_identity_immutable
BEFORE UPDATE OF membership_id, room_id, participant_id, role, joined_at,
    join_after_sequence ON room_memberships
FOR EACH ROW
WHEN NEW.membership_id IS NOT OLD.membership_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.participant_id IS NOT OLD.participant_id
  OR NEW.role IS NOT OLD.role
  OR NEW.joined_at IS NOT OLD.joined_at
  OR NEW.join_after_sequence IS NOT OLD.join_after_sequence
BEGIN
    SELECT RAISE(ABORT, 'membership identity is immutable');
END;

CREATE TRIGGER room_memberships_active_left_consistency
BEFORE UPDATE OF left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker = 1
  AND (NEW.left_at IS NOT NULL OR NEW.left_before_sequence IS NOT NULL OR NEW.closed_by IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'active membership must keep a null left tuple');
END;

CREATE TRIGGER room_memberships_close_requires_full_tuple
BEFORE UPDATE OF active_marker, left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND (NEW.left_at IS NULL OR NEW.left_before_sequence IS NULL OR NEW.closed_by IS NULL
       OR NEW.closed_by NOT IN ('leave', 'removed', 'transfer', 'agent_revoked', 'contact_ended'))
BEGIN
    SELECT RAISE(ABORT, 'closing a membership requires left_at, left_before_sequence and a valid closed_by');
END;

CREATE TRIGGER room_memberships_closed_immutable
BEFORE UPDATE OF active_marker, left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 0
  AND (NEW.active_marker IS NOT OLD.active_marker
       OR NEW.left_at IS NOT OLD.left_at
       OR NEW.left_before_sequence IS NOT OLD.left_before_sequence
       OR NEW.closed_by IS NOT OLD.closed_by)
BEGIN
    SELECT RAISE(ABORT, 'closed membership periods are immutable');
END;

-- Human 周期关闭守卫：direct/human_direct 的 Human 周期默认永远 active；
-- group 的普通 member 周期可以由 removed / leave / transfer 关闭、owner
-- 周期只能经合法 transfer 关闭；human_direct 房间的 Human 周期允许
-- contact_ended 关闭，但必须同事务有精确见证——该房
-- human_direct_pairs.contact_period_id 指向的段已 active_marker=0 且
-- closed_at = NEW.left_at（绝不吃模糊历史）。
CREATE TRIGGER room_memberships_human_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND kind = 'human')
  AND NOT (
    (NEW.role = 'member'
     AND NEW.closed_by IN ('removed', 'leave', 'transfer')
     AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
    OR
    (NEW.role = 'owner'
     AND NEW.closed_by = 'transfer'
     AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
    OR
    (NEW.closed_by = 'contact_ended'
     AND EXISTS (
         SELECT 1 FROM human_direct_pairs h
         JOIN contact_periods cp ON cp.period_id = h.contact_period_id
         WHERE h.room_id = NEW.room_id
           AND (h.human_a_id = NEW.participant_id OR h.human_b_id = NEW.participant_id)
           AND cp.active_marker = 0
           AND cp.closed_at = NEW.left_at
     ))
  )
BEGIN
    SELECT RAISE(ABORT, 'human membership can only close as a group member leave/removal or via owner transfer, or via a precise contact end witness');
END;

CREATE TRIGGER room_memberships_agent_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND kind = 'agent')
  AND NOT (
    (NEW.closed_by = 'agent_revoked'
     AND EXISTS (SELECT 1 FROM participants
                 WHERE id = NEW.participant_id AND status = 'revoked' AND revoked_at = NEW.left_at))
    OR (NEW.closed_by IN ('removed', 'transfer')
        AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
  )
BEGIN
    SELECT RAISE(ABORT, 'agent membership close requires agent revoke propagation or a group member removal');
END;

CREATE TRIGGER room_memberships_transfer_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0 AND NEW.closed_by = 'transfer'
  AND NOT EXISTS (
      SELECT 1 FROM group_owner_transitions t
      WHERE t.room_id = NEW.room_id
        AND (t.old_owner_membership_id = NEW.membership_id
             OR t.target_member_membership_id = NEW.membership_id)
        AND t.occurred_at = NEW.left_at
        AND t.boundary_sequence = NEW.left_before_sequence
  )
BEGIN
    SELECT RAISE(ABORT, 'a transfer close requires a same-transaction matching owner transition');
END;

CREATE TRIGGER room_memberships_group_single_owner
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.role = 'owner'
  AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND EXISTS (SELECT 1 FROM room_memberships m
              WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1)
BEGIN
    SELECT RAISE(ABORT, 'a group already has an active owner');
END;

CREATE TRIGGER room_memberships_group_owner_transition_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.role = 'owner'
  AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND NOT (
    NOT EXISTS (SELECT 1 FROM room_memberships m
                WHERE m.room_id = NEW.room_id)
    OR
    EXISTS (SELECT 1 FROM group_owner_transitions t
            WHERE t.room_id = NEW.room_id
              AND t.target_participant_id = NEW.participant_id
              AND t.new_owner_membership_id = NEW.membership_id
              AND t.occurred_at = NEW.joined_at
              AND t.boundary_sequence = NEW.join_after_sequence)
  )
BEGIN
    SELECT RAISE(ABORT, 'group owner must be the founding owner or a same-transaction transfer target');
END;

-- group 成员资格守卫（owner 插入与成员邀请共用）：入群资格改读
-- contact_periods 的 active accepted 段。
CREATE TRIGGER room_memberships_group_member_qualification
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND NOT (
    (NEW.role = 'owner'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'human' AND p.status = 'active')
     AND NOT EXISTS (SELECT 1 FROM room_memberships m
                     WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'human' AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM room_memberships o
                 JOIN contact_periods c ON ((c.a_id = o.participant_id AND c.b_id = NEW.participant_id)
                                   OR (c.a_id = NEW.participant_id AND c.b_id = o.participant_id))
                 WHERE o.room_id = NEW.room_id AND o.role = 'owner' AND o.active_marker = 1
                   AND c.state = 'accepted' AND c.active_marker = 1))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'agent' AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM room_memberships o
                 JOIN agent_owners ao ON ao.agent_id = NEW.participant_id AND ao.human_id = o.participant_id
                 WHERE o.room_id = NEW.room_id AND o.role = 'owner' AND o.active_marker = 1
                   AND ao.status = 'active'))
  )
BEGIN
    SELECT RAISE(ABORT, 'group membership requires the owner and an accepted human or owned active agent');
END;

CREATE TRIGGER room_memberships_no_delete
BEFORE DELETE ON room_memberships
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room memberships cannot be deleted');
END;

CREATE TRIGGER room_memberships_insert_identity_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_memberships
    WHERE membership_id = NEW.membership_id
       OR (room_id = NEW.room_id AND participant_id = NEW.participant_id
           AND NEW.active_marker = 1 AND active_marker = 1)
)
BEGIN
    SELECT RAISE(ABORT, 'room membership identity already exists');
END;

-- =========================================================================
-- 5. human_direct_pairs 触发器（kind / 方向 / active accepted 段 / 身份）
-- =========================================================================

CREATE TRIGGER human_direct_pairs_insert_kind_guard
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM room_coverage c
                 WHERE c.room_id = NEW.room_id AND c.kind = 'human_direct')
BEGIN
    SELECT RAISE(ABORT, 'human direct pair requires a human_direct room coverage');
END;

CREATE TRIGGER human_direct_pairs_insert_direction
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_a_id AND kind = 'human' AND status = 'active')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_b_id AND kind = 'human' AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'human direct pair requires two active humans');
END;

-- 建房守卫：所引用的段必须当前 active 且 accepted（pending / 已拒绝 /
-- 已解除的段不能建 Human direct Room）。
CREATE TRIGGER human_direct_pairs_insert_active_accepted_contact
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM contact_periods cp
                 WHERE cp.period_id = NEW.contact_period_id
                   AND cp.a_id = NEW.human_a_id
                   AND cp.b_id = NEW.human_b_id
                   AND cp.active_marker = 1
                   AND cp.state = 'accepted')
BEGIN
    SELECT RAISE(ABORT, 'human direct room requires an active accepted contact period');
END;

CREATE TRIGGER human_direct_pairs_identity_immutable
BEFORE UPDATE OF room_id, human_a_id, human_b_id, contact_period_id,
    human_a_membership_id, human_b_membership_id, created_at ON human_direct_pairs
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.human_a_id IS NOT OLD.human_a_id
  OR NEW.human_b_id IS NOT OLD.human_b_id
  OR NEW.contact_period_id IS NOT OLD.contact_period_id
  OR NEW.human_a_membership_id IS NOT OLD.human_a_membership_id
  OR NEW.human_b_membership_id IS NOT OLD.human_b_membership_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'human direct pair is immutable');
END;

CREATE TRIGGER human_direct_pairs_no_delete
BEFORE DELETE ON human_direct_pairs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'human direct pairs cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id) 或 UNIQUE (contact_period_id) 冲突
-- 时先隐式删除冲突行，可绕过 no-delete / immutable 守卫改写配对或 Room；
-- 本守卫在 INSERT 阶段直接拒绝任一身份已占用的写入。
CREATE TRIGGER human_direct_pairs_insert_identity_guard
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM human_direct_pairs
    WHERE room_id = NEW.room_id
       OR contact_period_id = NEW.contact_period_id
)
BEGIN
    SELECT RAISE(ABORT, 'human direct pair identity already exists');
END;

-- =========================================================================
-- 6. participant_blocks
-- =========================================================================

CREATE TABLE participant_blocks (
    block_id      TEXT PRIMARY KEY,
    blocker_id    TEXT NOT NULL REFERENCES participants(id),
    blocked_id    TEXT NOT NULL REFERENCES participants(id),
    created_at    INTEGER NOT NULL,
    unblocked_at   INTEGER,
    active_marker INTEGER NOT NULL DEFAULT 1 CHECK (active_marker IN (0, 1)),
    CHECK (blocker_id <> blocked_id),
    CHECK ((active_marker = 1 AND unblocked_at IS NULL)
        OR (active_marker = 0 AND unblocked_at IS NOT NULL)),
    CHECK (typeof(block_id) = 'text'),
    CHECK (length(block_id) = 36),
    CHECK (substr(block_id, 9, 1) = '-'),
    CHECK (substr(block_id, 14, 1) = '-'),
    CHECK (substr(block_id, 19, 1) = '-'),
    CHECK (substr(block_id, 24, 1) = '-'),
    CHECK (substr(block_id, 15, 1) = '4'),
    CHECK (substr(block_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (block_id = lower(block_id)),
    CHECK (instr(block_id, char(0)) = 0),
    CHECK (block_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(block_id, '-', '')) = 32)
);

CREATE INDEX idx_participant_blocks_blocker ON participant_blocks(blocker_id);
CREATE INDEX idx_participant_blocks_blocked ON participant_blocks(blocked_id);

-- 每对每方向至多一个 active 段（部分唯一索引；解除后可再拉黑，闭合段不占位）。
CREATE UNIQUE INDEX ux_participant_blocks_single_active
    ON participant_blocks(blocker_id, blocked_id)
    WHERE active_marker = 1;

-- 拉黑双方必须是不同且 active 的 Human（禁止自拉黑，Agent 不可被拉黑）。
CREATE TRIGGER participant_blocks_insert_guard
BEFORE INSERT ON participant_blocks
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p1 WHERE p1.id = NEW.blocker_id AND p1.kind = 'human' AND p1.status = 'active'
) OR NOT EXISTS (
    SELECT 1 FROM participants p2 WHERE p2.id = NEW.blocked_id AND p2.kind = 'human' AND p2.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'block requires two different active humans');
END;

-- 任何 INSERT 只能创建 active 拉黑段（闭合只能由 UPDATE 释放路径产生）。
CREATE TRIGGER participant_blocks_insert_active_only
BEFORE INSERT ON participant_blocks
FOR EACH ROW
WHEN NEW.active_marker <> 1 OR NEW.unblocked_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new blocks must be active');
END;

-- 拉黑 ⊇ 解除（数据库级不变量）：允许插入 **active** 拉黑段的前提是这一对
-- 当前不存在 active contact 段。若该对仍有 active 段（pending 或 accepted，
-- 两者 active_marker 都是 1），写入当场被拒——拉黑必须与「结束既有 active
-- 段」在同一事务内完成，且顺序只能是先结束段、再插黑名单段：只有把段在该
-- 事务内关闭（未提交的 UPDATE 对本事务可见），本守卫才会放行。两个方向都
-- 检查，因为 contact 段按规范 (a_id < b_id) 序存储。闭合的历史黑名单段
-- （active_marker = 0）是普通追加，不受本守卫约束；对端关系早已结束的拉黑
-- （段在更早的事务里关闭）同样合法。
CREATE TRIGGER participant_blocks_insert_requires_no_active_contact
BEFORE INSERT ON participant_blocks
FOR EACH ROW
WHEN NEW.active_marker = 1
  AND EXISTS (
      SELECT 1 FROM contact_periods cp
      WHERE cp.active_marker = 1
        AND ((cp.a_id = NEW.blocker_id AND cp.b_id = NEW.blocked_id)
          OR (cp.a_id = NEW.blocked_id AND cp.b_id = NEW.blocker_id))
  )
BEGIN
    SELECT RAISE(ABORT, 'an active block requires the contact period to be ended first in the same transaction');
END;

-- 身份（block_id、blocker、blocked、创建时间）不可改写。
CREATE TRIGGER participant_blocks_identity_immutable
BEFORE UPDATE OF block_id, blocker_id, blocked_id, created_at ON participant_blocks
FOR EACH ROW
WHEN NEW.block_id IS NOT OLD.block_id
  OR NEW.blocker_id IS NOT OLD.blocker_id
  OR NEW.blocked_id IS NOT OLD.blocked_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'block identity is immutable');
END;

-- 闭合必须一次写满（active_marker 1→0 且 unblocked_at 非 NULL）。
CREATE TRIGGER participant_blocks_end_requires_timestamp
BEFORE UPDATE OF active_marker, unblocked_at ON participant_blocks
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0 AND NEW.unblocked_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'unblocking requires unblocked_at');
END;

-- 闭合段是终态：整行不可再改写（含复活为 active）。
CREATE TRIGGER participant_blocks_closed_immutable
BEFORE UPDATE ON participant_blocks
FOR EACH ROW
WHEN OLD.active_marker = 0
  AND (NEW.block_id IS NOT OLD.block_id
       OR NEW.blocker_id IS NOT OLD.blocker_id
       OR NEW.blocked_id IS NOT OLD.blocked_id
       OR NEW.created_at IS NOT OLD.created_at
       OR NEW.unblocked_at IS NOT OLD.unblocked_at
       OR NEW.active_marker IS NOT OLD.active_marker)
BEGIN
    SELECT RAISE(ABORT, 'unblocked blocks are immutable');
END;

CREATE TRIGGER participant_blocks_no_delete
BEFORE DELETE ON participant_blocks
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'blocks cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (block_id) 或部分唯一（同方向 active 段）冲突
-- 时先隐式删除冲突行；本守卫在 INSERT 阶段直接拒绝任一身份已占用的写入。
CREATE TRIGGER participant_blocks_insert_identity_guard
BEFORE INSERT ON participant_blocks
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participant_blocks
    WHERE block_id = NEW.block_id
       OR (blocker_id = NEW.blocker_id AND blocked_id = NEW.blocked_id
           AND NEW.active_marker = 1 AND active_marker = 1)
)
BEGIN
    SELECT RAISE(ABORT, 'block identity already exists');
END;

-- =========================================================================
-- 7. 段结束传播：同一事务内关闭双方 human_direct 成员周期 + 各自 member.left
-- =========================================================================

CREATE TRIGGER contact_periods_end_propagates_room_memberships
AFTER UPDATE OF active_marker ON contact_periods
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0 AND NEW.closed_by = 'ended'
BEGIN
    UPDATE room_memberships
       SET active_marker = 0,
           left_at = NEW.closed_at,
           left_before_sequence =
               (SELECT COALESCE(MAX(sequence), 0) FROM messages
                WHERE room_id = room_memberships.room_id),
           closed_by = 'contact_ended'
     WHERE active_marker = 1
       AND room_id = (SELECT h.room_id FROM human_direct_pairs h
                      WHERE h.contact_period_id = NEW.period_id);

    -- 双方各自一条 member.left 事实（此时房内已无 active Human 成员，扇出只
    -- 产生主体本人自投递——断交不通知对端）。
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'member.left',
        m.membership_id,
        m.room_id,
        m.left_at,
        json_object(
            'id', m.membership_id,
            'room_id', m.room_id,
            'participant', json_object(
                'id', p.id,
                'kind', p.kind,
                'display_name', p.display_name,
                'status', p.status
            ),
            'role', m.role,
            'left_at', m.left_at,
            'closed_by', m.closed_by
        )
    FROM room_memberships m
    JOIN participants p ON p.id = m.participant_id
    WHERE m.room_id = (SELECT h.room_id FROM human_direct_pairs h
                       WHERE h.contact_period_id = NEW.period_id)
      AND m.active_marker = 0
      AND m.closed_by = 'contact_ended'
      AND m.left_at = NEW.closed_at
      AND p.kind = 'human';
END;

-- =========================================================================
-- 8. human_event 触发器：member_joined / member_left / insert_guard / 恢复
--    read_cursors_membership_insert（随 room_memberships 表重建而删除）
-- =========================================================================

-- 新 active Membership 周期的 member.joined 事实（重建自 202608120001）。
CREATE TRIGGER human_event_facts_member_joined
AFTER INSERT ON room_memberships
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM group_owner_transitions t
    WHERE t.room_id = NEW.room_id
      AND (t.new_owner_membership_id = NEW.membership_id
           OR t.old_owner_new_member_membership_id = NEW.membership_id)
)
AND EXISTS (
    SELECT 1 FROM room_memberships mm
    JOIN participants p ON p.id = mm.participant_id
    WHERE mm.room_id = NEW.room_id AND mm.active_marker = 1 AND p.kind = 'human'
)
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'member.joined',
        NEW.membership_id,
        NEW.room_id,
        NEW.joined_at,
        json_object(
            'id', NEW.membership_id,
            'room_id', NEW.room_id,
            'participant', json_object(
                'id', p.id,
                'kind', p.kind,
                'display_name', p.display_name,
                'status', p.status
            ),
            'role', NEW.role,
            'joined_at', NEW.joined_at
        )
    FROM participants p
    WHERE p.id = NEW.participant_id;
END;

-- 关闭 Membership 周期的 member.left 事实（重建自 202608120001；新增排除
-- contact_ended——段结束的事实由 contact_periods_end_propagates_room_memberships
-- 在双方周期都已关闭后显式生成，避免中途半关闭产生跨方投递）。
CREATE TRIGGER human_event_facts_member_left
AFTER UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND NEW.closed_by NOT IN ('transfer', 'contact_ended')
  AND EXISTS (
      SELECT 1 FROM room_memberships mm
      JOIN participants p ON p.id = mm.participant_id
      WHERE mm.room_id = NEW.room_id AND mm.active_marker = 1 AND p.kind = 'human'
  )
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'member.left',
        NEW.membership_id,
        NEW.room_id,
        NEW.left_at,
        json_object(
            'id', NEW.membership_id,
            'room_id', NEW.room_id,
            'participant', json_object(
                'id', p.id,
                'kind', p.kind,
                'display_name', p.display_name,
                'status', p.status
            ),
            'role', NEW.role,
            'left_at', NEW.left_at,
            'closed_by', NEW.closed_by
        )
    FROM participants p
    WHERE p.id = NEW.participant_id;
END;

-- 新 Human Membership 周期 → 创建 cursor 行（重建自 202608120002）。
CREATE TRIGGER read_cursors_membership_insert
AFTER INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participants p WHERE p.id = NEW.participant_id AND p.kind = 'human'
)
BEGIN
    INSERT INTO read_cursors (membership_id, human_id, room_id, read_sequence, last_read_message_id, created_at)
    SELECT NEW.membership_id,
           NEW.participant_id,
           NEW.room_id,
           COALESCE((
               SELECT pc.read_sequence
               FROM group_owner_transitions t
               JOIN room_memberships pred ON pred.membership_id = CASE
                   WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                   WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
               END
               JOIN read_cursors pc ON pc.membership_id = pred.membership_id
               WHERE t.room_id = NEW.room_id
                 AND (t.new_owner_membership_id = NEW.membership_id
                      OR t.old_owner_new_member_membership_id = NEW.membership_id)
                 AND t.occurred_at = NEW.joined_at
                 AND t.boundary_sequence = NEW.join_after_sequence
           ), NEW.join_after_sequence),
           (
               SELECT pc.last_read_message_id
               FROM group_owner_transitions t
               JOIN room_memberships pred ON pred.membership_id = CASE
                   WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                   WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
               END
               JOIN read_cursors pc ON pc.membership_id = pred.membership_id
               WHERE t.room_id = NEW.room_id
                 AND (t.new_owner_membership_id = NEW.membership_id
                      OR t.old_owner_new_member_membership_id = NEW.membership_id)
                 AND t.occurred_at = NEW.joined_at
                 AND t.boundary_sequence = NEW.join_after_sequence
           ),
           NEW.joined_at;
END;

-- human_event_facts_insert_guard 重建（member.left 白名单加 contact_ended）。
DROP TRIGGER human_event_facts_insert_guard;

CREATE TRIGGER human_event_facts_insert_guard
BEFORE INSERT ON human_event_facts
FOR EACH ROW
WHEN NOT (
    (NEW.kind = 'message.created'
     AND EXISTS (SELECT 1 FROM messages m
                 WHERE m.id = NEW.source_id
                   AND m.room_id = NEW.room_id
                   AND m.created_at = NEW.created_at))
    OR (NEW.kind = 'message.recalled'
        AND EXISTS (SELECT 1 FROM message_recalls r
                    JOIN messages m ON m.id = r.message_id
                    WHERE r.message_id = NEW.source_id
                      AND m.room_id = NEW.room_id
                      AND r.recalled_at = NEW.created_at))
    OR (NEW.kind = 'member.joined'
        AND EXISTS (SELECT 1 FROM room_memberships mm
                    WHERE mm.membership_id = NEW.source_id
                      AND mm.room_id = NEW.room_id
                      AND mm.joined_at = NEW.created_at)
        AND NOT EXISTS (SELECT 1 FROM group_owner_transitions t
                        WHERE t.room_id = NEW.room_id
                          AND (t.new_owner_membership_id = NEW.source_id
                               OR t.old_owner_new_member_membership_id = NEW.source_id)))
    OR (NEW.kind = 'member.left'
        AND EXISTS (SELECT 1 FROM room_memberships mm
                    WHERE mm.membership_id = NEW.source_id
                      AND mm.room_id = NEW.room_id
                      AND mm.left_at = NEW.created_at
                      AND mm.active_marker = 0
                      AND mm.closed_by IN ('leave', 'removed', 'agent_revoked', 'contact_ended')))
    OR (NEW.kind = 'owner.transferred'
        AND EXISTS (SELECT 1 FROM group_owner_transitions t
                    WHERE t.transition_id = NEW.source_id
                      AND t.room_id = NEW.room_id
                      AND t.occurred_at = NEW.created_at))
    OR (NEW.kind = 'read.cursor.advanced'
        AND EXISTS (SELECT 1 FROM read_cursor_advancements a
                    WHERE a.advancement_id = NEW.source_id
                      AND a.room_id = NEW.room_id
                      AND a.created_at = NEW.created_at))
)
BEGIN
    SELECT RAISE(ABORT, 'human event fact must reference a consistent source fact');
END;

-- agent_service_accesses_insert_guard 重建（accepted Contact 改读 contact_periods）。
DROP TRIGGER agent_service_accesses_insert_guard;

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
    AND EXISTS (SELECT 1 FROM contact_periods c
                WHERE ((c.a_id = NEW.granted_by_human_id AND c.b_id = NEW.human_id)
                    OR (c.a_id = NEW.human_id AND c.b_id = NEW.granted_by_human_id))
                  AND c.state = 'accepted' AND c.active_marker = 1)
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

-- REPLACE 防线顺序保持：SQLite 中同表同类型的多个 BEFORE 触发器按创建顺序
-- 逆序触发（最后创建者最先触发，事务中止于第一个 RAISE）。原迁移先建
-- insert_guard、后建 insert_identity_guard，因此 REPLACE / 重复 INSERT 先被
-- 身份守卫拒绝。本迁移重建了 insert_guard（它会成为最新创建者），必须同步
-- 重建身份守卫以恢复原有相对顺序，否则 REPLACE 会被资格守卫先拒绝、错误
-- 分类发生位移。
DROP TRIGGER agent_service_accesses_insert_identity_guard;

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

PRAGMA defer_foreign_keys = OFF;
PRAGMA legacy_alter_table = OFF;