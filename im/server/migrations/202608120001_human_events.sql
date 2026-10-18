-- Celira Core V1：Human Durable Event + per-Human delivery 切片。
--
-- 契约：CORE_V1_HUMAN_EVENTS.md
-- 两层规范化事实：
-- - human_event_facts：shared human event fact——每个 source occurrence 只
--   存一份：fact_id（UUID v4 文本主键）、kind、source_id、room_id、
--   created_at、snapshot（发生时已授权的公开快照 JSON，SQLite JSON1 由本
--   迁移的真实测试证明可用）；(kind, source_id) 唯一，不允许 message body /
--   snapshot 按 recipient N 份复制；
-- - human_event_deliveries：per-Human delivery——human_id + 自己范围内从
--   1 开始单调唯一的 sequence + shared fact 引用；(human_id, sequence)
--   主键、(human_id, fact_id) 唯一（同一 fact 对同一 Human 最多一条
--   delivery）；delivery 不复制任何正文或快照。
--
-- kind 与 source 对应：
-- - 'message.created'：source = messages.id；所有普通 Message（Human 消息
--   与 Completion 写回的 Agent 消息）都进入 Human Event；created_at 必须
--   等于 source Message created_at；
-- - 'message.recalled'：source = message_recalls.message_id；撤回事实成立时
--   一条（撤回扩展契约 CORE_V1_MESSAGE_RECALL.md §9；事实触发器与投递
--   窄例外由迁移 011 扩展/重建）；created_at = recalled_at；目标 = 该
--   Message 的 message.created 既有 delivery 集合（含撤回时已离群的原接收
--   者、不含后来加入者）；
-- - 'member.joined'：source = room_memberships.membership_id；每条新
--   active Membership 周期（含 direct/human_direct/group 奠基周期）一条；
--   created_at = joined_at；目标 = 加入后的 Room Human 集合（含本人）；
-- - 'member.left'：source = room_memberships.membership_id；每条关闭周期
--   （leave / removed / agent_revoked）一条；created_at = left_at；目标 =
--   关闭后仍在 Room 的 Human；主体是 Human 时还必须给本人一条自己的
--   member.left；
-- - 'owner.transferred'：source = group_owner_transitions.transition_id；
--   每次群主转让一条；created_at = occurred_at；目标 = 转让时点的 Room
--   Human 成员；四周期切换不产生 joined/left 噪音；
-- - 'read.cursor.advanced'：source = read_cursor_advancements.advancement_id；
--   每次 cursor 真实推进一条；created_at = advancement 时间；目标 = 推进者
--   本人（迁移 010 定向扇出；本迁移通用扇出排除该 kind）；
-- - closed_by='transfer' 的关闭与转让打开的周期不产生 joined/left 事实。
--
-- 触发链（不依赖同表外其他 AFTER trigger 的执行顺序）：
--   source 表 AFTER 触发器 → INSERT human_event_facts（事实守卫在同一
--   事务内证明 source 真实且字段一致）→ human_event_facts AFTER INSERT
--   触发器把该 fact 的 delivery 扇出到当时授权 Human（member.left 另补
--   主体本人的自投递）→ 同一事务 commit 才成立；任何一步失败整体回滚。
--
-- 守卫总览：
-- - fact：INSERT 必须引用真实 source（kind/source/room/created_at 一致，
--   joined 额外拒绝转让周期；read.cursor.advanced 由 migration 010 的
--   read_cursor_advancements 事实支撑——其 kind/source/room/created_at 必须
--   匹配真实 advancement 行）；全字段不可 UPDATE、不可 DELETE、REPLACE
--   以任一身份冲突时在插入阶段直接拒绝；
-- - delivery：sequence 必须等于该 Human 当前最大 sequence + 1（跳号、
--   重复一律拒绝）；target 守卫要求该 Human 存在一条覆盖 fact 时点的
--   Membership 周期（joined_at <= created_at <= left_at 或 left_at 为空；
--   member.left 主体本人由自己刚关闭的周期覆盖）；全字段不可
--   UPDATE、不可 DELETE、REPLACE 拒绝；
-- - read.cursor.advanced 的 delivery 只投递推进者本人：本迁移的通用扇出
--   排除该 kind，定向扇出由 migration 010 在 human_event_facts AFTER
--   INSERT 触发器中按 advancement.human_id 完成；认证通道/WebSocket 连接
--   状态、服务端 ACK、attempt、cursor 绝不写入本迁移的表。
--
-- 本迁移不修改 migration 001–008。旧开发数据库按项目破坏式收敛策略直接丢弃
-- 重建（不提供可升级旧库的兼容路径），migration 009 只对新建库生效：不回填
-- 既有 Message/Membership/Transition 的 Human Event（只对新事实生效）。
-- 本迁移由 migration 010 扩展 kind 白名单与 read.cursor.advanced 定向扇出
-- 守卫（见 202608120002_read_cursors.sql）。

CREATE TABLE human_event_facts (
    fact_id    TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN
                   ('message.created', 'message.recalled', 'member.joined', 'member.left',
                    'owner.transferred', 'read.cursor.advanced')),
    source_id  TEXT NOT NULL,
    room_id    TEXT NOT NULL REFERENCES rooms(id),
    created_at INTEGER NOT NULL,
    snapshot   TEXT NOT NULL,
    UNIQUE (kind, source_id),
    CHECK (typeof(fact_id) = 'text'),
    CHECK (length(fact_id) = 36),
    CHECK (substr(fact_id, 9, 1) = '-'),
    CHECK (substr(fact_id, 14, 1) = '-'),
    CHECK (substr(fact_id, 19, 1) = '-'),
    CHECK (substr(fact_id, 24, 1) = '-'),
    CHECK (substr(fact_id, 15, 1) = '4'),
    CHECK (substr(fact_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (fact_id = lower(fact_id)),
    CHECK (instr(fact_id, char(0)) = 0),
    CHECK (fact_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(fact_id, '-', '')) = 32),
    CHECK (typeof(kind) = 'text'),
    CHECK (typeof(source_id) = 'text'),
    CHECK (typeof(room_id) = 'text'),
    CHECK (typeof(created_at) = 'integer'),
    CHECK (typeof(snapshot) = 'text'),
    CHECK (instr(snapshot, char(0)) = 0),
    CHECK (json_valid(snapshot))
);

CREATE INDEX idx_human_event_facts_room ON human_event_facts(room_id);
CREATE INDEX idx_human_event_facts_source ON human_event_facts(source_id);

CREATE TABLE human_event_deliveries (
    human_id TEXT NOT NULL REFERENCES participants(id),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    fact_id  TEXT NOT NULL REFERENCES human_event_facts(fact_id),
    PRIMARY KEY (human_id, sequence),
    UNIQUE (human_id, fact_id),
    CHECK (typeof(human_id) = 'text'),
    CHECK (typeof(sequence) = 'integer'),
    CHECK (typeof(fact_id) = 'text')
);

CREATE INDEX idx_human_event_deliveries_fact ON human_event_deliveries(fact_id);

-- 共享事实守卫：任何直接 SQL 写入必须证明 source 真实存在且字段一致——
-- message.created 要求 messages 行匹配 room/created_at；message.recalled 的
-- 分支由迁移 011 重建本触发器时加入（需要 message_recalls 见证，本迁移
-- 时点该表尚不存在）；member.joined 要求真实 active 周期（joined_at 一致）
-- 且不是转让打开的周期（转让在 current transaction 中已有匹配
-- transition）；member.left 要求真实关闭周期（left_at 一致、closed_by 为
-- leave/removed/agent_revoked，transfer 关闭不产生事实）；owner.transferred
-- 要求真实 transition（occurred_at 一致）。
CREATE TRIGGER human_event_facts_insert_guard
BEFORE INSERT ON human_event_facts
FOR EACH ROW
WHEN NOT (
    (NEW.kind = 'message.created'
     AND EXISTS (SELECT 1 FROM messages m
                 WHERE m.id = NEW.source_id
                   AND m.room_id = NEW.room_id
                   AND m.created_at = NEW.created_at))
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
                      AND mm.closed_by IN ('leave', 'removed', 'agent_revoked')))
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
    -- message.recalled 分支由迁移 011 重建本触发器时追加。
)
BEGIN
    SELECT RAISE(ABORT, 'human event fact must reference a consistent source fact');
END;

-- 共享事实身份防改写：REPLACE 以主键 (fact_id) 或 UNIQUE (kind, source_id)
-- 冲突时先隐式删除冲突行，可绕过 no-delete / immutable 守卫改写事实；本
-- 守卫在 INSERT 阶段直接拒绝任一身份已占用的写入（普通重复 INSERT 同样被
-- 拒）。本守卫与插入守卫彼此独立。
CREATE TRIGGER human_event_facts_insert_identity_guard
BEFORE INSERT ON human_event_facts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM human_event_facts
    WHERE fact_id = NEW.fact_id
       OR (kind = NEW.kind AND source_id = NEW.source_id)
)
BEGIN
    SELECT RAISE(ABORT, 'human event fact identity already exists');
END;

-- 共享事实一经提交不可改写（等值自赋值不触发）。
CREATE TRIGGER human_event_facts_immutable
BEFORE UPDATE ON human_event_facts
FOR EACH ROW
WHEN NEW.fact_id IS NOT OLD.fact_id
  OR NEW.kind IS NOT OLD.kind
  OR NEW.source_id IS NOT OLD.source_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.snapshot IS NOT OLD.snapshot
BEGIN
    SELECT RAISE(ABORT, 'human event facts are immutable');
END;

-- 共享事实不允许硬删除。
CREATE TRIGGER human_event_facts_no_delete
BEFORE DELETE ON human_event_facts
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'human event facts cannot be deleted');
END;

-- delivery sequence 守卫：新 sequence 必须等于该 Human 当前最大 sequence
-- + 1；跳号、重复或非 next sequence 一律拒绝（配合 PK 与 UNIQUE）。
CREATE TRIGGER human_event_deliveries_sequence_must_be_next
BEFORE INSERT ON human_event_deliveries
FOR EACH ROW
WHEN NEW.sequence <> (
    SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries
    WHERE human_id = NEW.human_id
)
BEGIN
    SELECT RAISE(ABORT, 'human event delivery sequence must be the next available for the human');
END;

-- delivery target 守卫：投递目标必须存在一条 Membership 周期覆盖该 fact
-- 发生时点（joined_at <= created_at <= left_at，或周期未关闭）——正规流下
-- 扇出在 fact 创建的事务内执行，插入时点即发生时点；直接 SQL 无法把 fact
-- 投递给「发生时不在 Room」的 Human（其周期不覆盖 fact 时点），也无法把
-- member.left 投递给非主体本人（主体本人由自己刚关闭的周期覆盖；已在
-- fact 时点授权的目标在正规流下已有 delivery，(human_id, fact_id) 唯一
-- 拒绝重复）。同一写入同时违反多个守卫时先返回哪条数据库错误不属于契约。
--
-- 时间戳前提：服务端所有时间戳（joined_at / created_at / left_at /
-- occurred_at）由进程内单调时钟（UtcMillis::now()，系统墙钟回拨时冻结
-- 在已记录最大值，绝不回拨）产生，因此后记录的 created_at/left_at/
-- occurred_at 恒 >= 先前记录的 joined_at——正规 API 写入恒满足本守卫，
-- 不会因并发取时/时钟回拨被反向 abort；守卫自身不引入容差，直接 SQL
-- 伪造或回拨时间戳（created_at 早于目标 joined_at）仍被拒绝，授权窗口
-- 不被削弱。
CREATE TRIGGER human_event_deliveries_target_guard
BEFORE INSERT ON human_event_deliveries
FOR EACH ROW
WHEN NOT (
    EXISTS (
        -- 常规规则：投递目标必须存在一条 Membership 周期覆盖该 fact 时点
        -- （joined_at <= created_at <= left_at，或周期未关闭）。message.recalled
        -- 排除在常规规则之外：其目标严格等于 created 既有 delivery 集合
        -- （见下方窄例外），防止撤回时 active 的后来加入者被直接 SQL 投递。
        SELECT 1
        FROM human_event_facts f
        JOIN room_memberships mm ON mm.room_id = f.room_id AND mm.participant_id = NEW.human_id
        WHERE f.fact_id = NEW.fact_id
          AND f.kind <> 'message.recalled'
          AND mm.joined_at <= f.created_at
          AND (mm.left_at IS NULL OR f.created_at <= mm.left_at)
    )
    OR EXISTS (
        -- 撤回扩展窄例外（契约 CORE_V1_MESSAGE_RECALL.md §9）：message.recalled
        -- 的投递目标严格等于该 Message 的 message.created 既有 delivery
        -- 集合——撤回时已离群的原 created 接收者仍形成 delivery（其周期不
        -- 覆盖 recall fact 时点，但以 created 既有 delivery 为授权见证），
        -- 后来加入者不在 created 集合内、不得投递；这是唯一例外，其余
        -- kind 的 target 规则不变。
        SELECT 1
        FROM human_event_facts f
        JOIN human_event_facts c
          ON c.kind = 'message.created' AND c.source_id = f.source_id
        JOIN human_event_deliveries cd ON cd.fact_id = c.fact_id
        WHERE f.fact_id = NEW.fact_id AND f.kind = 'message.recalled'
          AND cd.human_id = NEW.human_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'human event delivery requires a membership period covering the fact time or a recalled-fact target copied from the created deliveries');
END;

-- delivery 身份防改写：REPLACE 以主键 (human_id, sequence) 或 UNIQUE
-- (human_id, fact_id) 冲突时先隐式删除冲突行；本守卫在 INSERT 阶段直接
-- 拒绝任一身份已占用的写入。
CREATE TRIGGER human_event_deliveries_insert_identity_guard
BEFORE INSERT ON human_event_deliveries
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM human_event_deliveries
    WHERE (human_id = NEW.human_id AND sequence = NEW.sequence)
       OR (human_id = NEW.human_id AND fact_id = NEW.fact_id)
)
BEGIN
    SELECT RAISE(ABORT, 'human event delivery identity already exists');
END;

-- delivery 一经提交不可改写。
CREATE TRIGGER human_event_deliveries_immutable
BEFORE UPDATE ON human_event_deliveries
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.sequence IS NOT OLD.sequence
  OR NEW.fact_id IS NOT OLD.fact_id
BEGIN
    SELECT RAISE(ABORT, 'human event deliveries are immutable');
END;

-- delivery 不允许硬删除。
CREATE TRIGGER human_event_deliveries_no_delete
BEFORE DELETE ON human_event_deliveries
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'human event deliveries cannot be deleted');
END;

-- 新普通 Message 的 message.created 事实（direct / human_direct / group /
-- Agent 回复共用）：Message 行成立时若 Room 存在 Human 成员（此时点授权
-- 可见），在同一事务内生成一条共享事实；snapshot 是发生时点公开快照
-- （id/room_id/actor 当前资料/body/reply_to_message_id/created_at/recalled_at；
-- 不含内部 Message sequence——公共协议不暴露任何 sequence，唯一耐久快照在
-- 事实生成时只固化公共字段，禁止读取时裁剪；
-- reply_to_message_id 直接来自同一 messages 行的不可变列，无 target 时
-- json_object 固化 JSON null；recalled_at 创建时恒为 JSON null，撤回扩展
-- （CORE_V1_MESSAGE_RECALL.md §7.2/§7.3）下由迁移 011 的 message_recalls
-- 同事务触发器受守卫单向变形为 body=null、recalled_at=首次撤回时点，
-- 其余键精确保留——本触发器之外不存在任何快照修改路径；Mention 属同事务
-- 伴随事实，不进入快照，客户端从权威 Room History 读取 Mention）；幂等
-- 重放（无第二次 INSERT）不触发。fact_id 为 SQLite 内生成的 UUID v4。
CREATE TRIGGER human_event_facts_message_created
AFTER INSERT ON messages
FOR EACH ROW
WHEN EXISTS (
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
        'message.created',
        NEW.id,
        NEW.room_id,
        NEW.created_at,
        json_object(
            'id', NEW.id,
            'room_id', NEW.room_id,
            'actor', json_object(
                'id', p.id,
                'kind', p.kind,
                'display_name', p.display_name,
                'status', p.status
            ),
            'body', NEW.body,
            'reply_to_message_id', NEW.reply_to_message_id,
            'created_at', NEW.created_at,
            'recalled_at', NULL
        )
    FROM participants p
    WHERE p.id = NEW.actor_id;
END;

-- 新 active Membership 周期的 member.joined 事实（direct / human_direct /
-- group 奠基与邀请/重入共用）：周期打开时若 Room 存在 Human 成员，生成
-- 一条共享事实；转让事务打开的新 owner / 旧 owner member 周期由
-- group_owner_transitions 精确见证，当前事务已存在匹配 transition，因此
-- 跳过（四周期切换不产生 joined/left 噪音）；普通重入不匹配任何 transition
-- 正常生成。
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

-- 关闭 Membership 周期的 member.left 事实（leave / removed /
-- agent_revoked 共用；closed_by='transfer' 的转让关闭不产生事实）：周期
-- 关闭后若 Room 仍有 Human 成员，生成一条共享事实；主体本人的自投递由
-- human_event_deliveries 扇出触发器补齐。
CREATE TRIGGER human_event_facts_member_left
AFTER UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0 AND NEW.closed_by <> 'transfer'
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

-- 群主转让的 owner.transferred 事实：transition 行成立（此时旧 owner 与
-- 目标 member 仍 active，转让前后 Human 成员集合相同）时生成一条共享
-- 事实；四周期切换不产生 joined/left 事实。
CREATE TRIGGER human_event_facts_owner_transferred
AFTER INSERT ON group_owner_transitions
FOR EACH ROW
WHEN EXISTS (
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
        'owner.transferred',
        NEW.transition_id,
        NEW.room_id,
        NEW.occurred_at,
        json_object(
            'id', NEW.transition_id,
            'room_id', NEW.room_id,
            'old_owner', json_object(
                'id', po.id,
                'kind', po.kind,
                'display_name', po.display_name,
                'status', po.status
            ),
            'new_owner', json_object(
                'id', pn.id,
                'kind', pn.kind,
                'display_name', pn.display_name,
                'status', pn.status
            ),
            'occurred_at', NEW.occurred_at
        )
    FROM participants po
    JOIN participants pn ON pn.id = NEW.target_participant_id
    WHERE po.id = NEW.old_owner_participant_id;
END;

-- delivery 扇出：共享事实成立后，在同一事务内把 delivery 投递给
-- 「该 fact 时点授权」的 Human——
-- 第一段：Room 当前（即事实时点）active 的 Human 成员（message.created /
-- member.joined / member.left / owner.transferred 的目标集合）；
-- read.cursor.advanced 与 message.recalled 排除在通用扇出之外：前者只投
-- 推进者本人（migration 010 定向扇出）；后者严格复制 message.created 既有
-- delivery 集合（migration 011 定向扇出）——若本段不排除，会把撤回事实
-- 错误投递给撤回时 active 的后来加入者，且 (human_id, fact_id) 唯一约束
-- 会与定向投递冲突使事务失败；
-- 第二段：member.left 的主体本人（Human 时）——主体周期刚关闭，不在第一
-- 段集合中；其自投递由关闭周期覆盖 fact 时点（left_at = created_at，
-- target 守卫 inclusive）而合法；若主体同时仍在 Room（不可能：同
-- Room+Participant 至多一个 active 周期），NOT EXISTS 防重复。
-- 每个 Human 的 sequence 为该 Human 当前最大 sequence + 1（扇出集合内
-- 每个 Human 至多一行，单写者事务内顺序确定）。
CREATE TRIGGER human_event_deliveries_fanout
AFTER INSERT ON human_event_facts
FOR EACH ROW
BEGIN
    INSERT INTO human_event_deliveries (human_id, sequence, fact_id)
    SELECT mm.participant_id,
           (SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries d
            WHERE d.human_id = mm.participant_id),
           NEW.fact_id
    FROM room_memberships mm
    JOIN participants p ON p.id = mm.participant_id
    WHERE mm.room_id = NEW.room_id AND mm.active_marker = 1 AND p.kind = 'human'
      AND NEW.kind NOT IN ('read.cursor.advanced', 'message.recalled');

    INSERT INTO human_event_deliveries (human_id, sequence, fact_id)
    SELECT s.participant_id,
           (SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries d
            WHERE d.human_id = s.participant_id),
           NEW.fact_id
    FROM room_memberships s
    JOIN participants p ON p.id = s.participant_id
    WHERE NEW.kind = 'member.left'
      AND s.membership_id = NEW.source_id
      AND p.kind = 'human'
      AND NOT EXISTS (SELECT 1 FROM human_event_deliveries d2
                      WHERE d2.fact_id = NEW.fact_id AND d2.human_id = s.participant_id);
END;
