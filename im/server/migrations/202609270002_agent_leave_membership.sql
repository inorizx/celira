-- =========================================================================
-- Agent 退群（M2 窄解冻切片，2026-09-27 主人批准；口径「能退，规则统一」）
-- =========================================================================
--
-- 症状：closed_by='leave' 本来就在词表里、Human 早就能用（POST
-- /v1/rooms/{room_id}/leave），而 Agent 用不了——agent_close_guard 只放行
-- agent_revoked（撤销传播）与群里的 removed/transfer。
--
-- 判据（不对称只许来自来历或处境）：这条限制没有来历或处境上的理由 ⇒ 是债。
-- Agent 被单方面放进房间（create_direct_room(human, agent_id)，无同意步骤），
-- 却只能被撤销——「在房里」这件事对 Agent 是单向的。本迁移补这笔债。
--
-- 规则（一处）：**非 owner 的成员周期，可以由它自己以 closed_by='leave'
-- 关闭**——不分房 kind、不分参与者 kind。
-- 例外（一处）：human_direct 房的出口归**关系模型**（contact_ended + 精确
-- 见证）——那间房的成员周期是一段关系的投影，不是「我在一个房里」。
-- owner 一律先转让（既有规则，本迁移不动）。
--
-- 本迁移只重建一道守卫：agent_close_guard 的 NOT(...) 里加 leave 分支，
-- 其余两个分支逐字保留。事件侧零改动（human_event_facts_member_left 触发器
-- 本来就与参与者 kind 无关；human_event_facts_insert_guard 的 member.left
-- 白名单本来就含 'leave'）。

DROP TRIGGER room_memberships_agent_close_guard;

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
    -- 【2026-09-27 新增】成员自己退出：任何房 kind，只要不是 human_direct
    -- （human_direct 的出口是关系出口，不是 leave）。owner 走不到这里——
    -- 存储层先以 409 owner_transfer_required 拒绝（本分支的 role='member'
    -- 是同一规则的兜底）。
    OR (NEW.closed_by = 'leave'
        AND NEW.role = 'member'
        AND EXISTS (SELECT 1 FROM rooms
                    WHERE id = NEW.room_id AND kind <> 'human_direct'))
  )
BEGIN
    SELECT RAISE(ABORT, 'agent membership close requires agent revoke propagation, a group member removal, or self leave');
END;
