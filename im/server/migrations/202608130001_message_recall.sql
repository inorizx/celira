-- Celira Core V1：Message Recall（消息撤回）切片。
--
-- 契约：CORE_V1_MESSAGE_RECALL.md（唯一权威契约）
--
-- 事实模型：
-- - message_recalls：每条 Message 至多一条的撤回终态公共事实。message_id
--   （PK/FK→messages）、recalled_by_participant_id（必须等于 Message.actor，
--   即原发送者本人）、recalled_at（SQLite 毫秒 statement-time，客户端不可
--   提交，显式传值由 guard 拒绝）。messages 行不改不删：body/sequence/
--   reply_to_message_id/idempotency_key/created_at 均保留。
-- - recalled_at 与撤回窗口（created_at <= now <= created_at + 120000ms）在
--   同一条 INSERT 语句内裁决：guard 在 BEFORE INSERT 中比较
--   NEW.recalled_at 与 CAST(unixepoch('subsec')*1000 AS INTEGER)（同 statement
--   time 恒等）以及 message.created_at 边界；应用层 INSERT ... SELECT 以同一
--   表达式生成 recalled_at 并在 WHERE 中做同一窗口裁决（超窗返回 0 行 →
--   409 message_recall_window_expired），guard 是对直接 SQL 的独立兜底。
--   系统时钟可信是部署前提。
-- - 撤回必须由满足权限与窗口的单一 INSERT 成立，并在同事务（BEGIN
--   IMMEDIATE）内原子收敛（message_recalls_after_insert_converges）：
--   1. Human Event fact 'message.recalled'（source_id = message_id，
--      created_at = recalled_at，快照最小集 message_id/room_id/recalled_at；
--      fact INSERT guard 需 message_recalls 见证）；
--   2. 该 fact 的 delivery 严格复制该 Message 的 message.created 既有
--      delivery 集合（human_event_deliveries_recalled_fanout；已离群的原
--      接收者仍投递、后来加入者不投——target guard 的窄例外已由 migration
--      009 冻结）；
--   3. message.created 快照受守卫单向变形为 tombstone（body=null、
--      recalled_at=首次撤回时点；id/room_id/actor/reply_to_message_id/
--      created_at 精确保留；human_event_facts_immutable 在本迁移重建为唯一
--      例外——允许的新快照必须与 OLD 经唯一 body=null + recalled_at=见证值
--      json_set 变换后的完整 JSON 精确相等，无见证、反向恢复、增删/重排
--      键、嵌套篡改、改其他键、二次变形一律拒绝）；
--   4. Agent Event 'message.recalled'（target 严格复制该 Message 已存在的
--      message.created target 集合，created_at = recalled_at；per-Agent
--      sequence 跨 kind 共享、严格 next；agent_events_insert_guard 在本迁移
--      重建加入 recalled 分支）。
-- - 撤回后禁止新建回复该 target：messages_reply_target_guard 在本迁移重建，
--   排除存在 message_recalls 的 target（应用层 validate_reply_target 同谓词
--   预检，统一 400 invalid_reply_target，不泄露是否因撤回）。
-- - 撤回先提交后：completions_insert_guard 在本迁移重建，拒绝对
--   message.recalled Event 本身的、或 source Message 已撤回的 Event 的新
--   Completion INSERT（既有 Completion 的同结果重放由应用层先裁决，返回
--   既有终态；本 guard 是独立兜底，统一映射 409 completion_source_recalled）。
-- - 本迁移不新增附件、编辑、管理代撤、ACK/Lease/Runtime 或第二套撤回
--   幂等表。

CREATE TABLE message_recalls (
    message_id                 TEXT PRIMARY KEY REFERENCES messages(id),
    recalled_by_participant_id TEXT NOT NULL REFERENCES participants(id),
    recalled_at                INTEGER NOT NULL,
    CHECK (typeof(message_id) = 'text'),
    CHECK (instr(message_id, char(0)) = 0),
    CHECK (typeof(recalled_by_participant_id) = 'text'),
    CHECK (instr(recalled_by_participant_id, char(0)) = 0),
    CHECK (typeof(recalled_at) = 'integer')
);

CREATE INDEX idx_message_recalls_recalled_by
    ON message_recalls(recalled_by_participant_id);

-- 撤回 INSERT 守卫（单一 statement 内裁决权限与时间，fail closed）：
-- - recalled_at 必须等于该语句的 SQLite 毫秒 statement-time（显式传旧/未来/
--   任意值一律拒绝；同语句内 unixepoch('subsec') 恒等，应用层
--   INSERT ... SELECT 也以同一表达式生成）；
-- - recalled_by_participant_id 必须等于 source Message 的 actor（代撤拒绝）；
-- - Room / Participant / Membership 在撤回时点必须 active；
-- - 窗口：created_at <= recalled_at <= created_at + 120000（边界恰等成立）。
CREATE TRIGGER message_recalls_insert_guard
BEFORE INSERT ON message_recalls
FOR EACH ROW
WHEN NOT (
    NEW.recalled_at = CAST(unixepoch('subsec') * 1000 AS INTEGER)
    AND EXISTS (
        SELECT 1
        FROM messages m
        JOIN rooms r ON r.id = m.room_id
        JOIN room_memberships mm
          ON mm.room_id = m.room_id AND mm.participant_id = NEW.recalled_by_participant_id
        JOIN participants p ON p.id = NEW.recalled_by_participant_id
        WHERE m.id = NEW.message_id
          AND m.actor_id = NEW.recalled_by_participant_id
          AND r.status = 'active'
          AND mm.active_marker = 1
          AND p.status = 'active'
          AND NEW.recalled_at >= m.created_at
          AND NEW.recalled_at <= m.created_at + 120000
    )
)
BEGIN
    SELECT RAISE(ABORT, 'message recall requires the author, an active room membership and the 120s window with statement-time recalled_at');
END;

-- 撤回事实一经提交不可改写（等值自赋值不触发；不重算 recalled_at）。
CREATE TRIGGER message_recalls_immutable
BEFORE UPDATE ON message_recalls
FOR EACH ROW
WHEN NEW.message_id IS NOT OLD.message_id
  OR NEW.recalled_by_participant_id IS NOT OLD.recalled_by_participant_id
  OR NEW.recalled_at IS NOT OLD.recalled_at
BEGIN
    SELECT RAISE(ABORT, 'message recalls are immutable');
END;

-- 撤回事实不允许硬删除。
CREATE TRIGGER message_recalls_no_delete
BEFORE DELETE ON message_recalls
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'message recalls cannot be deleted');
END;

-- 身份防改写：REPLACE / INSERT OR REPLACE 以主键 (message_id) 冲突时先隐式
-- 删除冲突行，可绕过 no-delete / immutable 守卫重建或改写既有撤回事实
-- （还会二次触发收敛副作用）；本守卫在 INSERT 阶段直接拒绝已占用的写入，
-- 普通重复 INSERT 同样被拒。
CREATE TRIGGER message_recalls_insert_identity_guard
BEFORE INSERT ON message_recalls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM message_recalls WHERE message_id = NEW.message_id
)
BEGIN
    SELECT RAISE(ABORT, 'message recall identity already exists');
END;

-- ── 重建 agent_events_insert_guard：加入 kind='message.recalled' 分支 ──────
-- 撤回 Event 的 INSERT 守卫：必须存在精确 message_recalls 见证（message_id
-- 匹配、created_at = recalled_at），且该 Message 对该 target 已存在
-- message.created Event（target 严格复制 created 集合，不按撤回时的
-- mention/member 动态重算；没有 created Event 的 Message——如 Agent 回复的
-- reply Message——不生成任何 message.recalled Event）。
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

-- ── 重建 human_event_facts_insert_guard：加入 kind='message.recalled' ──────
-- 分支：fact 必须由精确 message_recalls 见证（message_id 匹配、room 一致、
-- created_at = recalled_at）。
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
)
BEGIN
    SELECT RAISE(ABORT, 'human event fact must reference a consistent source fact');
END;

-- ── 重建 human_event_facts_immutable：唯一快照修改例外 ─────────────────────
-- 唯一允许的修改：message.created 快照在存在精确 message_recalls 见证时的
-- 一次 active shape → exact tombstone 变形（body 从 JSON text 变 JSON null、
-- recalled_at 变为该 Recall 的首次时点）。防线以**完整 JSON 文本精确相等**
-- 收口：允许的新快照必须与 OLD 快照经唯一变换
--   json_set(OLD.snapshot, '$.body', NULL, '$.recalled_at', <witness>)
-- 的完整结果逐字相等（键集合与顺序、嵌套结构、任何值都不得有差异）。
-- 生产收敛触发器（见下 message_recalls_after_insert_converges）以同一
-- json_set 表达式与键序生成该变换，因此守卫放行的路径与唯一写路径逐字节
-- 一致。防线：
-- - 无 Recall 见证（EXISTS 为空）→ 拒绝；
-- - 反向恢复 / 二次变形（OLD.body 已是 null）→ OLD body 非 text 条件失败
--   → 拒绝；
-- - 增加/删除/重排任何键、改动任何值（含 actor 等嵌套对象内部、body 之外
--   的一切）→ NEW.snapshot IS json_set(...) 整体比较失败 → 拒绝；
-- - UPDATE/DELETE 其他 fact/kind、其他字段修改（fact_id/kind/source_id/
--   room_id/created_at）→ 身份列比较失败 → 拒绝；
-- - 等值自赋值 UPDATE 不触发（NEW.snapshot IS OLD.snapshot 分支）。
-- 全部比较用 COALESCE/IS 消除 NULL 传播：SQLite 触发器 WHEN 表达式中
-- NULL 视为不触发，任何可空比较都必须显式归一（json_extract/json_type
-- 对缺失键返回 NULL；本守卫中 json_set 输入为 NOT NULL 的 snapshot 文本，
-- IS 比较结果恒为 0/1，不会静默放行）。
DROP TRIGGER human_event_facts_immutable;

CREATE TRIGGER human_event_facts_immutable
BEFORE UPDATE ON human_event_facts
FOR EACH ROW
WHEN NOT (
    NEW.fact_id IS OLD.fact_id
    AND NEW.kind IS OLD.kind
    AND NEW.source_id IS OLD.source_id
    AND NEW.room_id IS OLD.room_id
    AND NEW.created_at IS OLD.created_at
    AND (
        NEW.snapshot IS OLD.snapshot
        OR (
            OLD.kind = 'message.created'
            AND COALESCE(json_type(OLD.snapshot, '$.body'), '') = 'text'
            AND COALESCE(json_type(NEW.snapshot, '$.body'), '') = 'null'
            AND EXISTS (
                -- 精确见证 + 完整 JSON 精确相等：新快照必须等于 OLD 经唯一
                -- 变换 json_set(OLD, '$.body', NULL, '$.recalled_at', witness)
                -- 的完整结果（witness 由 message_recalls 提供，message_id
                -- 匹配）。无见证、recalled_at 不符、增加/删除/重排任何键
                -- 或改动任何嵌套值都会使整体 IS 比较失败。逐键比较曾允许
                -- json_set(body=NULL, recalled_at=witness, $.forged_extra=...)
                -- 之类变造 shape，本重建以整体相等收口。
                SELECT 1 FROM message_recalls r
                WHERE r.message_id = NEW.source_id
                  AND NEW.snapshot IS json_set(
                      OLD.snapshot, '$.body', NULL, '$.recalled_at', r.recalled_at
                  )
            )
        )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'human event facts are immutable');
END;

-- ── 重建 messages_reply_target_guard：排除已撤回 target ────────────────────
-- 撤回后禁止新建回复该 target（契约 CORE_V1_MESSAGE_RECALL.md 8）：target
-- 存在 message_recalls 事实时按 invalid_reply_target 统一拒绝，与未知/跨
-- Room/不可见同一错误面，不泄露是否因撤回；应用层 validate_reply_target
-- 以同一谓词预检（400 invalid_reply_target），本守卫是对直接 SQL 的独立
-- 兜底。
DROP TRIGGER messages_reply_target_guard;

CREATE TRIGGER messages_reply_target_guard
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NEW.reply_to_message_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1
      FROM messages t
      WHERE t.id = NEW.reply_to_message_id
        AND t.room_id = NEW.room_id
        AND t.id <> NEW.id
        AND NOT EXISTS (
            SELECT 1 FROM message_recalls r WHERE r.message_id = t.id
        )
        AND EXISTS (
            SELECT 1 FROM room_memberships mm
            WHERE mm.room_id = NEW.room_id AND mm.participant_id = NEW.actor_id
              AND mm.join_after_sequence < t.sequence
              AND (mm.left_before_sequence IS NULL OR t.sequence <= mm.left_before_sequence)
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'reply target must be a same-room historical message visible to the actor');
END;

-- ── 重建 completions_insert_guard：撤回先提交后拒绝新 Completion ───────────
-- 撤回先提交后，该 source Event（message.created）的任何首次 Completion 与
-- 对 message.recalled Event 本身的 Completion 一律被拒绝（统一映射 409
-- completion_source_recalled，无 Message/Completion/sequence 副作用）；
-- 既有 Completion 的同结果幂等重放由应用层先裁决返回既有终态，不进入本
-- INSERT；本守卫是对直接 SQL 的独立兜底。
DROP TRIGGER completions_insert_guard;

CREATE TRIGGER completions_insert_guard
BEFORE INSERT ON completions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM agent_events e
    WHERE e.id = NEW.event_id
      AND e.target_agent_id = NEW.target_agent_id
      AND e.kind = 'message.created'
      AND NOT EXISTS (
          SELECT 1 FROM message_recalls r WHERE r.message_id = e.message_id
      )
      AND (
          NEW.outcome <> 'reply'
          OR (
              NEW.reply_message_id IS NOT NULL
              AND EXISTS (
                  SELECT 1
                  FROM messages m
                  WHERE m.id = NEW.reply_message_id
                    AND m.actor_id = NEW.target_agent_id
                    AND m.room_id = (
                        SELECT room_id FROM messages WHERE id = e.message_id
                    )
              )
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'completion requires the event target match and a reply message from the target agent in the event room');
END;

-- ── 撤回收敛触发器：message_recalls INSERT 的唯一原子落点 ──────────────────
-- 与 Recall 事实同一语句、同一事务内完成全部收敛（任何一步失败整体回滚）：
-- 1. Human fact message.recalled（仅当该 Message 存在 message.created 事实
--    时生成——delivery 继承自 created 既有集合，见下方定向扇出）；
-- 2. message.created 快照受守卫单向变形（body=null、recalled_at=首次时点）；
-- 3. Agent message.recalled Event（target = 该 Message 已存在的 created
--    target 集合，per-Agent sequence 跨 kind 共享、严格 next）。
-- 本触发器不依赖同表其他 AFTER 触发器的执行顺序；重放（无第二次 INSERT）
-- 不触发；失败不反转已提交结果（触发器失败使整个语句与外层事务回滚）。
CREATE TRIGGER message_recalls_after_insert_converges
AFTER INSERT ON message_recalls
FOR EACH ROW
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.recalled',
        NEW.message_id,
        m.room_id,
        NEW.recalled_at,
        json_object(
            'message_id', NEW.message_id,
            'room_id', m.room_id,
            'recalled_at', NEW.recalled_at
        )
    FROM messages m
    WHERE m.id = NEW.message_id
      AND EXISTS (SELECT 1 FROM human_event_facts c
                  WHERE c.kind = 'message.created' AND c.source_id = NEW.message_id);

    UPDATE human_event_facts
    SET snapshot = json_set(
            snapshot,
            '$.body', NULL,
            '$.recalled_at', NEW.recalled_at
        )
    WHERE kind = 'message.created' AND source_id = NEW.message_id;

    INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.recalled',
        e.target_agent_id,
        (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
         WHERE target_agent_id = e.target_agent_id),
        e.message_id,
        NEW.recalled_at
    FROM agent_events e
    WHERE e.kind = 'message.created' AND e.message_id = NEW.message_id;
END;

-- ── message.recalled 定向扇出：严格复制 message.created 既有 delivery ─────
-- 以 created 既有 delivery 集合为授权见证（契约 CORE_V1_MESSAGE_RECALL.md
-- §9）：撤回时已离群的原 created 接收者仍形成 delivery（target guard 的
-- 窄例外已由 migration 009 冻结）、后来加入者不在 created 集合内不投；
-- 通用扇出（migration 009）已排除该 kind，避免按撤回时 active 成员动态
-- 扩散。每个 Human 的 sequence 为该 Human 当前最大 sequence + 1。
CREATE TRIGGER human_event_deliveries_recalled_fanout
AFTER INSERT ON human_event_facts
FOR EACH ROW
WHEN NEW.kind = 'message.recalled'
BEGIN
    INSERT INTO human_event_deliveries (human_id, sequence, fact_id)
    SELECT cd.human_id,
           (SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries d
            WHERE d.human_id = cd.human_id),
           NEW.fact_id
    FROM human_event_facts c
    JOIN human_event_deliveries cd ON cd.fact_id = c.fact_id
    WHERE c.kind = 'message.created' AND c.source_id = NEW.source_id;
END;
