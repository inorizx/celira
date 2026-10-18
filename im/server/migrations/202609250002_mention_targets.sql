-- Celira Core V1：Mention 目标放开（group 房间内任意 active 成员）窄切片。
--
-- 契约：CORE_V1_MENTION_TARGETS.md（唯一权威契约）
--
-- Mention 目标从「active Agent 成员」放宽为「group 消息所在 Room 中
-- active 的任意成员（Human 或 Agent）」；@ 人成为合法事实。房间类型边界
-- 不放开：direct / human_direct 仍然要求 mentions 为空。事件语义不放开：
-- 只有目标 participant 是 active Agent 时才生成 agent_event，@ Human
-- 不产生任何事件（不新增事件 kind、不新增 human event）。
--
-- 1. mentions_insert_guard 重建：去掉目标必须是 Agent 的判据
--    （p.kind = 'agent'），保留 group-only、active Membership 周期
--    （mm.active_marker = 1）与目标 participant 当前 active
--    （p.status = 'active'）三条判据。RAISE 文案同步改为「active member
--    target」，不再提 agent。
--
-- 2. mentions_after_insert_generates_agent_event 重建：加 Agent 过滤——
--    只有 NEW.target_agent_id 是 active Agent 时才生成 agent_event。
-- 该过滤同时使 @ Human 根本不进入 agent_events，从而不触发
--    agent_events_insert_guard 的 mention 分支（该分支要求
--    a.kind = 'agent'，不满足则 RAISE(ABORT) 使整条 Message 事务回滚）；
--    两处守卫都靠这一条过滤避开，缺一不可。
--
-- 本迁移只重建这两个触发器，不改表结构、不改其它守卫。

DROP TRIGGER IF EXISTS mentions_insert_guard;

CREATE TRIGGER mentions_insert_guard
BEFORE INSERT ON mentions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM messages m
    JOIN rooms r ON r.id = m.room_id
    JOIN room_memberships mm
      ON mm.room_id = m.room_id AND mm.participant_id = NEW.target_agent_id AND mm.active_marker = 1
    JOIN participants p ON p.id = NEW.target_agent_id
    WHERE m.id = NEW.message_id
      AND r.kind = 'group'
      AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'mention requires a group message and an active member target');
END;

DROP TRIGGER IF EXISTS mentions_after_insert_generates_agent_event;

CREATE TRIGGER mentions_after_insert_generates_agent_event
AFTER INSERT ON mentions
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM participants a
             WHERE a.id = NEW.target_agent_id AND a.kind = 'agent' AND a.status = 'active')
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
