-- Celira Core V1：read cursor / 未读切片。
--
-- 契约：CORE_V1_READ_STATE.md
--
-- 事实模型（两层，独立于 Membership 行本身，不建立可漂移 unread counter）：
-- - read_cursors：可变 cursor，持久身份归属一条 Human Membership 周期
--   （membership_id 主键，与 room_memberships 一对一；Agent Membership
--   不得插入 cursor）。每周期一条：内部 read_sequence（只用于单调性与
--   未读计算，不对外暴露）+ 公开 last_read_message_id（可 null）。初始
--   boundary 例外：新/重入周期 read_sequence = join_after_sequence 且
--   last_read_message_id = NULL；closed 周期冻结，不能继续推进；历史
--   cursor 不删除、不复用、不改绑。
-- - read_cursor_advancements：不可变推进事实——每次真实推进一条
--   （advancement_id UUID v4 主键、Human、Room、Membership、前后位置、
--   Message 与时间一致）；它是 Human Event kind 'read.cursor.advanced'
--   的 source（kind/source_id/room_id/created_at 精确匹配由 migration
--   009 的 fact INSERT guard 与迁移 010 的 fact 触发器共同保证）。
--
-- 触发链：
--   1. room_memberships AFTER INSERT（Human 周期）→ 创建 cursor 行：
--      - 普通加入/重入/奠基：read_sequence = NEW.join_after_sequence、
--        last_read_message_id = NULL；
--      - 群主转让打开的新 owner / 旧 owner member 周期：由同一事务内
--        已插入的 group_owner_transitions 精确见证（occurred_at =
--        joined_at、boundary_sequence = join_after_sequence），继承被关闭
--        前序周期（target_member / old_owner）cursor 的 read_sequence 与
--        last_read_message_id——转让是连续成员关系，两位 Human 的未读连续；
--  2. 真实推进唯一写路径 = INSERT read_cursor_advancements（RoomStore.
--     advance_read_cursor，BEGIN IMMEDIATE，单语句）：
--     INSERT read_cursor_advancements（守卫当场证明 from_sequence = 当前
--     cursor、Message 同 Room 且对本人历史可见、to > from）→ 同一语句内
--     AFTER INSERT 触发器链自动完成其余全部步骤：
--     (a) read_cursors_advancement_applies：UPDATE read_cursors 单调推进
--         （UPDATE 守卫要求精确 advancement 见证——membership_id /
--         human_id / room_id / from_sequence = OLD / to_sequence = NEW /
--         last_read_message_id 全等；并保留 active、单调、Message 同 Room
--         + sequence 一致 + 窗口可见防线。任意无见证的直接 UPDATE，即便
--         Message/sequence 完全合法，也被拒绝）；
--     (b) human_event_facts_read_cursor_advanced：生成 read.cursor.advanced
--         fact；
--     (c) human_event_deliveries_read_cursor_fanout：fact 定向扇出（只投
--         推进者本人）。
--     (a)(b) 只依赖已插入的 advancement 事实，不依赖同表其他 AFTER
--     触发器执行顺序；任一步失败，整个 INSERT 语句与外层事务整体回滚，
--     不留下半事实；no-op / 失败整体回滚。
--
-- 守卫总览（fail closed，直接 SQL 不能绕过）：
-- - read_cursors：INSERT 只接受「Human 周期的 join boundary 初值」或
--   「同一事务 transition 精确见证的前序 cursor 继承」两种形状；身份
--   （membership_id）已存在拒绝（REPLACE 同防）；UPDATE 只允许
--   read_sequence / last_read_message_id 两列在存在精确 advancement
--   见证时单调推进——见证必须与 OLD/NEW 全等（membership_id、human_id、
--   room_id、from_sequence = OLD.read_sequence、to_sequence =
--   NEW.read_sequence、last_read_message_id = NEW.last_read_message_id），
--   且目标仍是同 Room、同 sequence、对本人历史可见的真实 Message，周期
--   仍 active（closed 冻结）；真实推进只能由 INSERT advancement 的
--   AFTER 触发器驱动，Rust 不再存在第二条 cursor 写路径；身份列不可改；
--   DELETE / REPLACE 拒绝；
-- - read_cursor_advancements：INSERT 必须 from = 当前 cursor、to > from、
--   last_read_message_id 与 to_sequence 指向同一真实同 Room Message 且
--   对本人历史可见；每 cursor 的 to_sequence / last_read_message_id 唯一
--   （单调推进使重复不可能）；整行不可 UPDATE / DELETE / REPLACE；
-- - fact 与 delivery 复用 migration 009 的共享守卫：kind 白名单已扩展
--   read.cursor.advanced；通用扇出排除该 kind，定向扇出只投 advancement
--   记录的 Human（其 active 周期覆盖 fact 时点，target 守卫通过）。
--
-- 本迁移只新增只服务 read state 的表与触发器，不修改 migration 001–008；
-- migration 009 的 kind 白名单 / fact guard / 通用扇出排除由本切片同步扩展
-- （同一破坏式收敛：旧开发数据一律丢弃，不回填 cursor）。

CREATE TABLE read_cursors (
    membership_id TEXT PRIMARY KEY REFERENCES room_memberships(membership_id),
    human_id      TEXT NOT NULL REFERENCES participants(id),
    room_id       TEXT NOT NULL REFERENCES rooms(id),
    read_sequence INTEGER NOT NULL CHECK (read_sequence >= 0),
    last_read_message_id TEXT REFERENCES messages(id),
    created_at    INTEGER NOT NULL,
    CHECK (typeof(membership_id) = 'text'),
    CHECK (typeof(human_id) = 'text'),
    CHECK (typeof(room_id) = 'text'),
    CHECK (typeof(read_sequence) = 'integer'),
    CHECK (last_read_message_id IS NULL OR typeof(last_read_message_id) = 'text'),
    CHECK (typeof(created_at) = 'integer'),
    CHECK (last_read_message_id IS NULL OR read_sequence > 0)
);

CREATE INDEX idx_read_cursors_human ON read_cursors(human_id);
CREATE INDEX idx_read_cursors_room ON read_cursors(room_id);

-- cursor 行创建形状守卫：只接受两种合法来路——
-- (a) 新/重入/奠基的 Human Membership 周期：read_sequence 必须等于该
--     周期的 join_after_sequence 且 last_read_message_id 为 NULL，且该周期
--     不是转让打开的周期（转让继承必须走 (b)）；
-- (b) 群主转让打开的新 owner / 旧 owner member 周期：必须由同一事务内
--     的 group_owner_transitions 精确见证（room、occurred_at = 周期的
--     joined_at、boundary_sequence = join_after_sequence、membership_id
--     匹配 transition 的 new_owner / old_owner_new_member 角色列），且
--     read_sequence / last_read_message_id 必须等于被关闭前序周期（target
--     member / old owner）cursor 的当前值——继承不能凭时间猜，只能由
--     transition 事实精确复制。
-- 直接 SQL 伪造：非 Human 周期 cursor、跳读（read_sequence 不等于
-- join_after 且无 transition 见证）、转让周期不继承前序值、Agent 周期
-- cursor 全部在此被拒。
CREATE TRIGGER read_cursors_insert_guard
BEFORE INSERT ON read_cursors
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM room_memberships mm
        JOIN participants p ON p.id = mm.participant_id
        WHERE mm.membership_id = NEW.membership_id
          AND mm.room_id = NEW.room_id
          AND mm.participant_id = NEW.human_id
          AND p.kind = 'human'
    )
    AND (
        (
            NEW.last_read_message_id IS NULL
            AND NEW.read_sequence = (
                SELECT join_after_sequence FROM room_memberships
                WHERE membership_id = NEW.membership_id
            )
            AND NOT EXISTS (
                SELECT 1 FROM group_owner_transitions t
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
            )
        )
        OR
        (
            NEW.read_sequence = (
                SELECT pc.read_sequence
                FROM group_owner_transitions t
                JOIN room_memberships mm ON mm.membership_id = NEW.membership_id
                JOIN room_memberships pred ON pred.membership_id = CASE
                    WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                    WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
                END
                JOIN read_cursors pc ON pc.membership_id = pred.membership_id
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
                  AND t.occurred_at = mm.joined_at
                  AND t.boundary_sequence = mm.join_after_sequence
            )
            AND NEW.last_read_message_id IS (
                SELECT pc.last_read_message_id
                FROM group_owner_transitions t
                JOIN room_memberships mm ON mm.membership_id = NEW.membership_id
                JOIN room_memberships pred ON pred.membership_id = CASE
                    WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                    WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
                END
                JOIN read_cursors pc ON pc.membership_id = pred.membership_id
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
                  AND t.occurred_at = mm.joined_at
                  AND t.boundary_sequence = mm.join_after_sequence
            )
        )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor must be a fresh join boundary or a same-transaction transfer inheritance of the predecessor cursor');
END;

-- cursor 身份防改写：REPLACE 以主键 (membership_id) 冲突时先隐式删除冲突
-- 行，可绕过 no-delete / immutable 守卫重建 cursor（把 closed 周期 cursor
-- 复活为 join boundary 或改写 last_read_message_id）；本守卫在 INSERT 阶段
-- 直接拒绝 membership_id 已存在的写入。
CREATE TRIGGER read_cursors_insert_identity_guard
BEFORE INSERT ON read_cursors
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM read_cursors WHERE membership_id = NEW.membership_id
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor identity already exists');
END;

-- cursor 身份列（归属周期、Human、Room、创建时间）一经创建不可改写；
-- 只有 read_sequence / last_read_message_id 走专属推进守卫。
CREATE TRIGGER read_cursors_identity_immutable
BEFORE UPDATE OF membership_id, human_id, room_id, created_at ON read_cursors
FOR EACH ROW
WHEN NEW.membership_id IS NOT OLD.membership_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'read cursor identity is immutable');
END;

-- cursor 推进守卫：read_sequence / last_read_message_id 只能同时单调推进，
-- 且必须满足——
-- 1. NEW.read_sequence > OLD.read_sequence（只前进，并发乱序自然收敛到
--    最高合法位置，由 BEGIN IMMEDIATE 单写者裁决）；
-- 2. 存在与该 UPDATE 全等的 advancement 见证：同 membership_id 的
--    read_cursor_advancements 行的 human_id / room_id 与 cursor 一致、
--    from_sequence = OLD.read_sequence、to_sequence = NEW.read_sequence、
--    last_read_message_id = NEW.last_read_message_id 精确匹配——真实推进
--    只能由 INSERT advancement 的 AFTER 触发器驱动，任意无见证的直接
--    UPDATE（即便 Message/sequence 形状完全合法）也被拒绝；
-- 3. last_read_message_id 非空且与 read_sequence 指向同一真实 Message
--    （同 Room、sequence 匹配）——last_read_message_id 与 read_sequence
--    必须一致，初始 boundary 例外只在 INSERT 允许 null；
-- 4. 归属 Membership 周期仍 active（closed 周期冻结，不能继续推进）；
-- 5. 目标 Message 落在该 Human 任一真实 Membership 周期窗口内
--    （join_after_sequence < sequence 且 left_before_sequence 为空或
--    sequence <= left_before_sequence）——首次加入前 / 离群期间 / 他 Room
--    的消息不能成为推进目标。
-- 直接 SQL 倒退、跳读（无真实 Message）、无见证推进、closed 周期推进、
-- 他 Room / 不可见 Message 全部在此被拒。
CREATE TRIGGER read_cursors_update_guard
BEFORE UPDATE OF read_sequence, last_read_message_id ON read_cursors
FOR EACH ROW
WHEN NOT (
    NEW.read_sequence > OLD.read_sequence
    AND NEW.last_read_message_id IS NOT NULL
    AND EXISTS (
        SELECT 1 FROM room_memberships mm
        WHERE mm.membership_id = NEW.membership_id AND mm.active_marker = 1
    )
    AND EXISTS (
        SELECT 1 FROM messages m
        WHERE m.id = NEW.last_read_message_id
          AND m.room_id = NEW.room_id
          AND m.sequence = NEW.read_sequence
    )
    AND EXISTS (
        SELECT 1 FROM room_memberships w
        WHERE w.room_id = NEW.room_id AND w.participant_id = NEW.human_id
          AND w.join_after_sequence < NEW.read_sequence
          AND (w.left_before_sequence IS NULL OR NEW.read_sequence <= w.left_before_sequence)
    )
    AND EXISTS (
        SELECT 1 FROM read_cursor_advancements a
        WHERE a.membership_id = NEW.membership_id
          AND a.human_id = NEW.human_id
          AND a.room_id = NEW.room_id
          AND a.from_sequence = OLD.read_sequence
          AND a.to_sequence = NEW.read_sequence
          AND a.last_read_message_id = NEW.last_read_message_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor can only advance monotonically via an exact advancement witness to a visible same-room message while its membership period is active');
END;

-- cursor 不允许硬删除（历史 cursor 不删除、不复用、不改绑）。
CREATE TRIGGER read_cursors_no_delete
BEFORE DELETE ON read_cursors
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'read cursors cannot be deleted');
END;

-- 不可变推进事实：每次真实推进一条；from_sequence = 推进前 cursor 位置、
-- to_sequence = 推进后位置、last_read_message_id = 目标 Message。
CREATE TABLE read_cursor_advancements (
    advancement_id      TEXT PRIMARY KEY,
    human_id            TEXT NOT NULL REFERENCES participants(id),
    room_id             TEXT NOT NULL REFERENCES rooms(id),
    membership_id       TEXT NOT NULL REFERENCES room_memberships(membership_id),
    from_sequence       INTEGER NOT NULL CHECK (from_sequence >= 0),
    to_sequence         INTEGER NOT NULL CHECK (to_sequence > 0),
    last_read_message_id TEXT NOT NULL REFERENCES messages(id),
    created_at          INTEGER NOT NULL,
    CHECK (to_sequence > from_sequence),
    CHECK (typeof(advancement_id) = 'text'),
    CHECK (length(advancement_id) = 36),
    CHECK (substr(advancement_id, 9, 1) = '-'),
    CHECK (substr(advancement_id, 14, 1) = '-'),
    CHECK (substr(advancement_id, 19, 1) = '-'),
    CHECK (substr(advancement_id, 24, 1) = '-'),
    CHECK (substr(advancement_id, 15, 1) = '4'),
    CHECK (substr(advancement_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (advancement_id = lower(advancement_id)),
    CHECK (instr(advancement_id, char(0)) = 0),
    CHECK (advancement_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(advancement_id, '-', '')) = 32),
    -- 同一 cursor 的推进位置与目标 Message 唯一（单调推进使重复不可能，
    -- 此处兜底直接 SQL 重放）。
    UNIQUE (membership_id, to_sequence),
    UNIQUE (membership_id, last_read_message_id)
);

CREATE INDEX idx_read_cursor_advancements_human
    ON read_cursor_advancements(human_id);
CREATE INDEX idx_read_cursor_advancements_room
    ON read_cursor_advancements(room_id);

-- advancement INSERT 守卫：必须从 cursor 当前真实位置推进——
-- 1. from_sequence 必须等于该 cursor 当前的 read_sequence（cursor 存在、
--    归属周期 active；伪造 from / 重放旧推进全部被拒）；
-- 2. last_read_message_id 与 to_sequence 指向同一真实 Message（同 Room）；
-- 3. 目标 Message 对该 Human 任一真实 Membership 周期窗口可见；
-- 4. to_sequence > from_sequence（表 CHECK 兜底）。
-- 合法 INSERT 的 AFTER 触发器自动推进 cursor 并原子生成 fact/delivery
-- （见 read_cursors_advancement_applies 与下方 fact 触发器）；
-- 直接 SQL 不能伪造推进事实，也不能无见证推进 cursor。
CREATE TRIGGER read_cursor_advancements_insert_guard
BEFORE INSERT ON read_cursor_advancements
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM read_cursors c
        JOIN room_memberships mm ON mm.membership_id = c.membership_id
        WHERE c.membership_id = NEW.membership_id
          AND c.human_id = NEW.human_id
          AND c.room_id = NEW.room_id
          AND c.read_sequence = NEW.from_sequence
          AND mm.active_marker = 1
    )
    AND EXISTS (
        SELECT 1 FROM messages m
        WHERE m.id = NEW.last_read_message_id
          AND m.room_id = NEW.room_id
          AND m.sequence = NEW.to_sequence
    )
    AND EXISTS (
        SELECT 1 FROM room_memberships w
        WHERE w.room_id = NEW.room_id AND w.participant_id = NEW.human_id
          AND w.join_after_sequence < NEW.to_sequence
          AND (w.left_before_sequence IS NULL OR NEW.to_sequence <= w.left_before_sequence)
    )
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor advancement must move from the current cursor to a visible same-room message');
END;

-- advancement 身份防改写：REPLACE 以主键 (advancement_id) 或任一 UNIQUE
-- 冲突时先隐式删除冲突行；本守卫在 INSERT 阶段直接拒绝任一身份已占用的
-- 写入（普通重复 INSERT 同样被拒）。
CREATE TRIGGER read_cursor_advancements_insert_identity_guard
BEFORE INSERT ON read_cursor_advancements
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM read_cursor_advancements
    WHERE advancement_id = NEW.advancement_id
       OR (membership_id = NEW.membership_id AND to_sequence = NEW.to_sequence)
       OR (membership_id = NEW.membership_id AND last_read_message_id = NEW.last_read_message_id)
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor advancement identity already exists');
END;

-- advancement 整行不可 UPDATE。
CREATE TRIGGER read_cursor_advancements_immutable
BEFORE UPDATE ON read_cursor_advancements
FOR EACH ROW
WHEN NEW.advancement_id IS NOT OLD.advancement_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.membership_id IS NOT OLD.membership_id
  OR NEW.from_sequence IS NOT OLD.from_sequence
  OR NEW.to_sequence IS NOT OLD.to_sequence
  OR NEW.last_read_message_id IS NOT OLD.last_read_message_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'read cursor advancements are immutable');
END;

-- advancement 不允许硬删除。
CREATE TRIGGER read_cursor_advancements_no_delete
BEFORE DELETE ON read_cursor_advancements
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'read cursor advancements cannot be deleted');
END;

-- 真实推进落地触发器：INSERT read_cursor_advancements 的唯一原子落点——
-- 在同一语句内自动、精确地把对应 cursor 推进到该 advancement 的
-- to_sequence / last_read_message_id。UPDATE 守卫要求精确见证
-- （membership/human/room/from = OLD/to = NEW/last_read_message_id 全等），
-- 合法 INSERT 恒满足；无见证的直接 UPDATE 无法绕过。
-- 本触发器与 human_event_facts_read_cursor_advanced 各自只依赖已插入的
-- advancement 事实，不依赖同表其他 AFTER 触发器的执行顺序；任一步失败
-- （UPDATE 守卫、fact 守卫或 delivery 守卫），整个 INSERT 语句与外层事务
-- 整体回滚，不留下半事实。
CREATE TRIGGER read_cursors_advancement_applies
AFTER INSERT ON read_cursor_advancements
FOR EACH ROW
BEGIN
    UPDATE read_cursors
    SET read_sequence = NEW.to_sequence,
        last_read_message_id = NEW.last_read_message_id
    WHERE membership_id = NEW.membership_id;
END;

-- 新 Human Membership 周期 → 创建 cursor 行：普通加入/重入/奠基
-- read_sequence = join_after_sequence、last_read_message_id = NULL；
-- 转让打开的新 owner / 旧 owner member 周期继承前序 cursor（由同一事务
-- 内已插入的 transition 精确见证，见 read_cursors_insert_guard 的 (b) 分支
-- 与上方触发链说明）。Agent 周期不产生 cursor（WHEN 限定 Human）。
-- 触发顺序：transfer 事务先插 transition、再关闭旧周期（cursor 保留）、
-- 再打开新周期——本触发器执行时前序 cursor 已冻结存在。
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

-- 真实推进 → read.cursor.advanced 共享 fact（每 advancement 一条）：
-- snapshot 只含 Room ID 与 last_read_message_id 等 read-state 必要稳定
-- 字段，不含 advancement id（advancement_id 是内部 source 身份，只在
-- facts.source_id 列存在，绝不进入公共快照/HTTP）、Message 正文、
-- sequence boundary、Membership 历史、Session/设备/token。fact INSERT
-- guard（migration 009）在同一个事务内证明 source 真实且
-- kind/source/room/created_at 一致。
CREATE TRIGGER human_event_facts_read_cursor_advanced
AFTER INSERT ON read_cursor_advancements
FOR EACH ROW
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'read.cursor.advanced',
        NEW.advancement_id,
        NEW.room_id,
        NEW.created_at,
        json_object(
            'room_id', NEW.room_id,
            'last_read_message_id', NEW.last_read_message_id
        );
END;

-- read.cursor.advanced 定向扇出：只投递推进者本人（advancement 记录的
-- human_id）；通用扇出（migration 009）已排除该 kind，避免向 Room 全部
-- Human 错误投递。推进者 active 周期覆盖 fact 时点，target 守卫通过。
CREATE TRIGGER human_event_deliveries_read_cursor_fanout
AFTER INSERT ON human_event_facts
FOR EACH ROW
WHEN NEW.kind = 'read.cursor.advanced'
BEGIN
    INSERT INTO human_event_deliveries (human_id, sequence, fact_id)
    SELECT a.human_id,
           (SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries d
            WHERE d.human_id = a.human_id),
           NEW.fact_id
    FROM read_cursor_advancements a
    WHERE a.advancement_id = NEW.source_id;
END;
