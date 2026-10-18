-- Celira Core V1：Durable Agent Event 切片。
--
-- 契约：CORE_V1_AGENT_EVENTS.md + CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md +
-- CORE_V1_MESSAGE_RECALL.md（撤回扩展：kind 列 + (kind, message_id,
-- target_agent_id) 身份，同一 per-Agent sequence 空间跨 kind 共享）
-- - agent_events 是"哪条已提交 Message 应交给哪个 Agent 处理"的耐久来源：
--   Event ID（UUID v4 文本主键）、kind（message.created / message.recalled）、
--   target Agent（引用 Participant）、该 Agent 自己范围内从 1 开始的正整数
--   sequence（跨 kind 共享，不因 kind 分列）、source Message ID、created_at；
--   (target_agent_id, sequence) 唯一、(kind, message_id, target_agent_id)
--   唯一（同一 Message 对同一 target 每个 kind 至多一条）；Event 所有字段
--   不可 UPDATE、Event 不可 DELETE；
-- - 同一 Event 表同时支持两种目标来源：
--   (a) 旧 Human–Agent Direct Room 自动目标：新普通 Message 的 Event 由
--       messages AFTER INSERT 触发器在同一事务内生成（source actor 必须
--       是该 Direct Room 配对的 Human，target 必须是同一配对的 Agent，
--       且 target Participant 与 Membership 周期当前 active（active_marker=1））；
--   (b) group 结构化 Mention 目标：Message 先插入、Mention 后插入（同一
--       事务），每条 Mention 由 mentions AFTER INSERT 触发器为被提到的
--       Agent 生成一条 Event；普通 group Message（无 Mention）与 Agent
--       回复（不携带 Mention）都不生成 Event，不会自触发；
-- - Event created_at 必须等于 source Message created_at；新 sequence 必须
--   等于该 target Agent 当前最大 sequence + 1（BEFORE INSERT 数据库守卫
--   直接拒绝跳号、重复与非 next sequence）；不用全局 sequence；
-- - 反向直接写入 agent_events 必须同时满足：source Message 是配对 Human
--   消息（direct 路径）或该消息的 Mention 记录（group 路径）、target 是
--   active Agent 且有 active Membership 周期（active_marker=1）、created_at 与 source Message
--   一致、sequence 为 next；UPDATE/DELETE 一律拒绝；
-- - REPLACE 防线：SQLite 的 INSERT OR REPLACE / REPLACE 在 id、
--   (target_agent_id, sequence) 或 (kind, message_id, target_agent_id) 任一身份
--   冲突时先隐式删除冲突行，默认 recursive_triggers=OFF 下不会触发
--   no-delete 触发器，可绕过 immutable / no-delete 守卫替换既有 Event；
--   BEFORE INSERT identity guard 在插入阶段直接拒绝任一身份已存在的写入
--   （普通重复 INSERT 同样被拒）；
-- - Binding/Credential 不参与 Event schema、生成条件或外键；Agent/
--   Binding/Credential revoke 不删除、不改写 Event 行（无撤销传播
--   触发器）；既有 Message 不回填；
-- - 撤回扩展（契约 CORE_V1_MESSAGE_RECALL.md §10，实现迁移 011）：
--   message.recalled 的 INSERT 守卫与触发器见
--   migrations/202608130001_message_recall.sql（本迁移只冻结 kind 列、
--   (kind, message_id, target_agent_id) 唯一身份与跨 kind 共享 sequence；
--   message.recalled 事件由 message_recalls INSERT 同事务触发器按该
--   Message 既有 message.created target 复制生成，created_at = recalled_at）；
-- - 本迁移不引入 Completion、ACK、持久 cursor、attempt、Lease、Runtime
--   或 Agent 私有事实。

CREATE TABLE agent_events (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL DEFAULT 'message.created'
                    CHECK (kind IN ('message.created', 'message.recalled')),
    target_agent_id TEXT NOT NULL REFERENCES participants(id),
    sequence        INTEGER NOT NULL CHECK (sequence > 0),
    message_id      TEXT NOT NULL REFERENCES messages(id),
    created_at      INTEGER NOT NULL,
    CHECK (typeof(id) = 'text'),
    CHECK (length(id) = 36),
    CHECK (substr(id, 9, 1) = '-'),
    CHECK (substr(id, 14, 1) = '-'),
    CHECK (substr(id, 19, 1) = '-'),
    CHECK (substr(id, 24, 1) = '-'),
    CHECK (substr(id, 15, 1) = '4'),
    CHECK (substr(id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (id = lower(id)),
    CHECK (instr(id, char(0)) = 0),
    CHECK (id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(id, '-', '')) = 32),
    CHECK (typeof(kind) = 'text'),
    CHECK (instr(kind, char(0)) = 0),
    UNIQUE (target_agent_id, sequence),
    UNIQUE (kind, message_id, target_agent_id)
);

CREATE INDEX idx_agent_events_target_sequence
    ON agent_events(target_agent_id, sequence);

-- sequence 守卫：新 sequence 必须等于该 target Agent 当前最大 sequence + 1；
-- 直接插入跳号、重复或非 next sequence 一律拒绝（配合 UNIQUE 约束）。
-- 本守卫与后续配对守卫彼此独立；同一写入同时违反多个守卫时，具体先返回
-- 哪条数据库错误不属于契约，正确性不得依赖同类 BEFORE trigger 的执行顺序。
CREATE TRIGGER agent_events_sequence_must_be_next
BEFORE INSERT ON agent_events
FOR EACH ROW
WHEN NEW.sequence <> (
    SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
    WHERE target_agent_id = NEW.target_agent_id
)
BEGIN
    SELECT RAISE(ABORT, 'agent event sequence must be the next available for the target agent');
END;

-- 直接写入守卫：Event 只允许合法来源之一——
-- (a) kind='message.created' 且 source Message 是该 Direct Room 配对的
--     Human 消息、target 是同一配对的 Agent，且 target Participant 与
--     Membership 周期当前 active（active_marker=1），created_at 必须等于
--     source Message created_at；配对 CHECK(human_id <> agent_id) 同时排除
--     target 等于 actor 自身；
-- (b) kind='message.created' 且 source Message 存在一条指向 target 的
--     Mention（该 Mention 已通过 005 的守卫：group 消息、active Agent
--     成员），created_at 必须等于 source Message created_at，且 target
--     当前仍为 active Agent 并保有 active Membership 周期（active_marker=1）；
-- (c) kind='message.recalled'：由迁移 011 连同本触发器整体重建（该分支
--     需要 message_recalls 事实见证，本迁移时点该表尚不存在），本迁移
--     只冻结 created 两分支。
-- 本守卫不依赖与 sequence 守卫之间的执行顺序。
CREATE TRIGGER agent_events_insert_guard
BEFORE INSERT ON agent_events
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM messages m
    JOIN direct_room_pairs p ON p.room_id = m.room_id AND p.human_id = m.actor_id
    WHERE NEW.kind = 'message.created'
      AND m.id = NEW.message_id
      AND p.agent_id = NEW.target_agent_id
      AND m.created_at = NEW.created_at
      AND EXISTS (SELECT 1 FROM participants a
                  WHERE a.id = p.agent_id AND a.kind = 'agent' AND a.status = 'active')
      AND EXISTS (SELECT 1 FROM room_memberships mm
                  WHERE mm.room_id = p.room_id
                    AND mm.participant_id = p.agent_id
                    AND mm.active_marker = 1)
)
AND NOT EXISTS (
    SELECT 1
    FROM mentions mnt
    JOIN messages m ON m.id = mnt.message_id
    WHERE NEW.kind = 'message.created'
      AND mnt.message_id = NEW.message_id
      AND mnt.target_agent_id = NEW.target_agent_id
      AND m.created_at = NEW.created_at
      AND EXISTS (SELECT 1 FROM participants a
                  WHERE a.id = NEW.target_agent_id AND a.kind = 'agent' AND a.status = 'active')
      AND EXISTS (SELECT 1 FROM room_memberships mm
                  WHERE mm.room_id = m.room_id
                    AND mm.participant_id = NEW.target_agent_id
                    AND mm.active_marker = 1)
)
BEGIN
    SELECT RAISE(ABORT, 'agent event requires a paired human message or a mention of an active target agent');
END;

-- Event 事实一经提交不可改写（等值自赋值不触发；kind 身份列一并不可改）。
CREATE TRIGGER agent_events_immutable
BEFORE UPDATE ON agent_events
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
  OR NEW.kind IS NOT OLD.kind
  OR NEW.target_agent_id IS NOT OLD.target_agent_id
  OR NEW.sequence IS NOT OLD.sequence
  OR NEW.message_id IS NOT OLD.message_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'agent events are immutable');
END;

-- Event 不允许硬删除。
CREATE TRIGGER agent_events_no_delete
BEFORE DELETE ON agent_events
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'agent events cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (id)、UNIQUE (target_agent_id, sequence)
-- 或 UNIQUE (kind, message_id, target_agent_id) 任一冲突时先隐式删除冲突
-- 行，可绕过 no-delete / immutable 守卫替换既有 Event（sequence 守卫只
-- 约束新 sequence，不能单独挡住 REPLACE 的删除并重建）；本守卫在 INSERT
-- 阶段直接拒绝任一身份已占用的写入。同一 Message 对同一 target 的
-- message.created 与 message.recalled 是不同 kind 身份，互不冲突。
-- 本守卫与 sequence 守卫 / 插入守卫彼此独立，同一写入同时违反多个约束时
-- 先返回哪条错误不属于契约。
CREATE TRIGGER agent_events_insert_identity_guard
BEFORE INSERT ON agent_events
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_events
    WHERE id = NEW.id
       OR (target_agent_id = NEW.target_agent_id AND sequence = NEW.sequence)
       OR (kind = NEW.kind AND message_id = NEW.message_id AND target_agent_id = NEW.target_agent_id)
)
BEGIN
    SELECT RAISE(ABORT, 'agent event identity already exists');
END;

-- 新普通 Message 的 Event 生成（direct 路径）：actor 是配对 Human、target
-- 配对 Agent 且 Participant/Membership 周期当前 active（active_marker=1）时，在同一事务内为该
-- Agent 分配下一条 sequence 并生成恰好一条 Event；target 非 active、
-- Agent actor（含 Completion 写回的 Agent Message）、幂等重放（无第二次
-- INSERT）与未知 Room 类型都不生成。Event ID 为 SQLite 内生成的 UUID v4
-- （版本 nibble 固定 4、variant nibble 取 8/9/a/b，全小写连字符）。
CREATE TRIGGER messages_after_insert_generates_agent_event
AFTER INSERT ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id AND p.human_id = NEW.actor_id
)
BEGIN
    INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.created',
        p.agent_id,
        (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
         WHERE target_agent_id = p.agent_id),
        NEW.id,
        NEW.created_at
    FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id AND p.human_id = NEW.actor_id
      AND EXISTS (SELECT 1 FROM participants a
                  WHERE a.id = p.agent_id AND a.kind = 'agent' AND a.status = 'active')
      AND EXISTS (SELECT 1 FROM room_memberships mm
                  WHERE mm.room_id = p.room_id
                    AND mm.participant_id = p.agent_id
                    AND mm.active_marker = 1);
END;

-- 新 Mention 的 Event 生成（group 路径）：Mention 与 source Message 同
-- 一事务（Message 先插入、Mention 后插入），每条 Mention 在被插入时
-- 为该被提到的 Agent 分配下一条 sequence 并生成恰好一条 Event；普通
-- group Message（无 Mention）与 Agent 回复（不携带 Mention）不触发；
-- Mention 的任何守卫失败都会使整个 Message 事务回滚。
CREATE TRIGGER mentions_after_insert_generates_agent_event
AFTER INSERT ON mentions
FOR EACH ROW
BEGIN
    INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.created',
        NEW.target_agent_id,
        (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
         WHERE target_agent_id = NEW.target_agent_id),
        NEW.message_id,
        (SELECT m.created_at FROM messages m WHERE m.id = NEW.message_id)
    WHERE EXISTS (SELECT 1 FROM messages m WHERE m.id = NEW.message_id);
END;
