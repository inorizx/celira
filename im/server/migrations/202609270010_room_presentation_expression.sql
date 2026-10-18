-- =========================================================================
-- 批次四·表达权第三刀：Agent 写自己在群里的呈现（2026-09-28 主人方向「表达权这种」）
-- =========================================================================
--
-- 症状：一支 Agent **不能写"自己在某个群里是什么样"**——那段呈现只能由该群 owner 代笔
--（守卫原文要求 published_by_id 是 `kind='human'` 的 active owner）。
--
-- 根因：与表达权前两刀同一条轴——判据写的是"谁管这间房"，而不是"这是谁的呈现"。
--      房呈现是**主体在某一间房里的样子**：它自己的那一份，归它自己。
--
-- 本迁移**只重建两道触发器**（不动表、不搬行、不新增持久对象 ⇒ 真家只需重编 + 重启）：
--
--   ① `room_presentation_versions_insert_guards`
--      发布者那一支由「该房 active owner 且是 Human」放开为**并集**：
--        · 该房的 **active owner**（**不再限 kind**——批次三第一刀已把 owner 判据
--          从 kind 换成"创建者"，这里那处 `p.kind='human'` 是当时的遗留），**或**
--        · **被呈现的那位主体自己**（`published_by_id = participant_id`）。
--      其余三支**逐字不动**：房必须是 active group、被呈现者必须是该房 active **Agent**
--      成员（`role='member'`）、目标必须在房内 active。
--
--   ② `room_presentation_put_idempotency_actor_human`
--      幂等记录 actor 与 ① 同形：active owner（不限 kind）**或**被呈现者自己。
--
-- 形状与批次三第③步、表达权前两刀**同一条**：**并集**——房 owner 的写权限一字不动，
-- 只是多了"它自己"这一支。
--
-- ⚠ 与存储层配套（同一刀）：`put_room_presentation_version` 的授权改成同形并集
--（`is_owner` 去掉 kind 判据 + 允许"发布者就是被呈现者"）。

-- ① 房呈现：owner（不限 kind）或主体自己。
DROP TRIGGER room_presentation_versions_insert_guards;

CREATE TRIGGER room_presentation_versions_insert_guards
BEFORE INSERT ON room_presentation_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r
    WHERE r.id = NEW.room_id AND r.kind = 'group' AND r.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.participant_id AND p.kind = 'agent' AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.room_id = NEW.room_id
      AND m.participant_id = NEW.participant_id
      AND m.role = 'member'
      AND m.active_marker = 1
)
OR NOT (
    EXISTS (
        SELECT 1
        FROM room_memberships o
        JOIN participants p ON p.id = o.participant_id
        WHERE o.room_id = NEW.room_id
          AND o.role = 'owner'
          AND o.active_marker = 1
          AND p.id = NEW.published_by_id
          AND p.status = 'active'
    )
    OR NEW.published_by_id = NEW.participant_id
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation requires a group room, an active agent member, and the active group owner or the participant itself as publisher');
END;

-- ② 幂等记录 actor：与 ① 同形。
DROP TRIGGER room_presentation_put_idempotency_actor_human;

CREATE TRIGGER room_presentation_put_idempotency_actor_human
BEFORE INSERT ON room_presentation_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id
      AND p.status = 'active'
      AND (p.kind = 'human' OR p.id = NEW.participant_id)
)
BEGIN
    SELECT RAISE(ABORT, 'room presentation idempotency actor must be an active human or the participant itself');
END;
