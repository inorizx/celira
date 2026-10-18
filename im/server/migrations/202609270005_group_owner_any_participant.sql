-- =========================================================================
-- 批次三·第一刀：owner = 创建者（2026-09-27 主人批准）
-- =========================================================================
--
-- 症状：公共世界不支持「没有 Human 在场的 Agent 空间」——一支 Agent **建不出任何房**，
-- 而"房主必须是 Human"这条判据在数据库里由**两道**守卫承担：
--   ① `room_memberships_group_member_qualification` 的 owner 分支要求 kind='human'
--      ⇒ Agent 既当不了奠基 owner，也接不了转让来的 owner；
--   ② `group_owner_transitions_insert_guard` 要求 old owner 与 target 都是 kind='human'
--      ⇒ 转让给 Agent 被拒。
--
-- 目标：**owner 是房内的治理角色（role），不是参与者种类（kind）**。任何 active 主体
-- 创建的房间，它就是 owner。
--
-- 本迁移**只重建这两道触发器**：不动表结构、不搬行、不新增任何持久对象。真家只需重编 +
-- 重启（不需要重置世界）。
--
-- ⚠ 这不是"放宽"，是把判据从 kind 换成事实：
--   · owner 分支仍要求**该主体当前 active**，且**该房当前没有别的 active owner**；
--   · member 分支仍要求**关系**——与 owner 之间有 accepted 联系人段，或该主体被 owner
--     以 active Agent Owner 关系拥有（两条关系各自的 kind 守卫仍在原处：
--     `contact_periods_insert_guard` 要求两端 Human，`agent_owners_insert_direction`
--     要求 human→agent）。写成"关系的并集"而不是"按 kind 分支"，是为了让**一条判据**
--     就能覆盖今后的关系类型（例如把 contact 泛化到任意两个 active 主体时，这道守卫
--     不需要再改一次）。
--
-- 迁移治理：本文件是 trigger-only，不涉及行搬迁；与 CORE_V1_IM_CONTRACT.md 的
-- 「旧开发数据整体丢弃重建」条款一致（真家本次**不需要**重新奠基）。

-- ── 1. 成员资格守卫（owner 插入与成员邀请共用）─────────────────────────────
-- 原文来自 `202609250001_contact_reversibility.sql`（§579），本迁移做三处改写：
--   owner 分支去掉 `p.kind = 'human'`（保留 active + 同房无其他 active owner）；
--   member 分支由「按 kind 二选一」改成「两条关系取并集」（判据集合逐条不变）；
--   member 分支**新增一条精确豁免**：该周期来自同一事务内精确匹配的 owner
--   transition（即"旧 owner 转让后被降级为 member"）。
--
-- 为什么需要那条豁免（施工时撞出来的真事实）：这条守卫管的是**入群资格**
-- （谁能被拉进房），而转让里那条 member 周期**不是入群**——那个人本来就在房里，
-- 只是角色从 owner 换成 member。原来的形状在"owner 只能是 Human"时恰好不会暴露，
-- 因为新 owner 一定是由旧 owner 邀请进来的（⇒ 两人必有 accepted 联系人段，
-- 降级也过得了资格判据）。owner 可以是 Agent 之后这条隐含前提不成立了：
-- Agent 名下的房里没有任何 Human 能满足"与 owner 有关系"，于是**转让给 Agent
-- 会当场撞守卫**。豁免写成"同一事务内精确匹配的 transition"（与 owner 那一侧的
-- `room_memberships_group_owner_transition_guard` 同一手法），不吃模糊历史；
-- 入群资格本身一个字没放宽。
DROP TRIGGER room_memberships_group_member_qualification;

CREATE TRIGGER room_memberships_group_member_qualification
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND NOT (
    (NEW.role = 'owner'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.status = 'active')
     AND NOT EXISTS (SELECT 1 FROM room_memberships m
                     WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM room_memberships o
                 WHERE o.room_id = NEW.room_id AND o.role = 'owner' AND o.active_marker = 1
                   AND (
                     EXISTS (SELECT 1 FROM contact_periods c
                             WHERE c.active_marker = 1 AND c.state = 'accepted'
                               AND ((c.a_id = o.participant_id AND c.b_id = NEW.participant_id)
                                 OR (c.a_id = NEW.participant_id AND c.b_id = o.participant_id)))
                     OR
                     EXISTS (SELECT 1 FROM agent_owners ao
                             WHERE ao.agent_id = NEW.participant_id
                               AND ao.human_id = o.participant_id
                               AND ao.status = 'active')
                   )))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM group_owner_transitions t
                 WHERE t.room_id = NEW.room_id
                   AND t.old_owner_participant_id = NEW.participant_id
                   AND t.old_owner_new_member_membership_id = NEW.membership_id
                   AND t.occurred_at = NEW.joined_at
                   AND t.boundary_sequence = NEW.join_after_sequence))
  )
BEGIN
    SELECT RAISE(ABORT, 'group membership requires an active owner and an accepted contact or an owned active agent');
END;

-- ── 2. 群主转让的插入守卫 ──────────────────────────────────────────────────
-- 原文来自 `202608110004_direct_rooms.sql`（§1122），本迁移只去掉两处 `p.kind = 'human'`
-- （保留 active、同房周期见证、自 PK 起的角色匹配、唯一 owner、不能是自己等全部判据）。
DROP TRIGGER group_owner_transitions_insert_guard;

CREATE TRIGGER group_owner_transitions_insert_guard
BEFORE INSERT ON group_owner_transitions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.kind = 'group'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.membership_id = NEW.old_owner_membership_id
      AND m.room_id = NEW.room_id
      AND m.participant_id = NEW.old_owner_participant_id
      AND m.role = 'owner'
      AND m.active_marker = 1
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.membership_id = NEW.target_member_membership_id
      AND m.room_id = NEW.room_id
      AND m.participant_id = NEW.target_participant_id
      AND m.role = 'member'
      AND m.active_marker = 1
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.old_owner_participant_id AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.target_participant_id AND p.status = 'active'
)
OR EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1
      AND m.membership_id <> NEW.old_owner_membership_id
)
BEGIN
    SELECT RAISE(ABORT, 'group owner transition requires an active old owner and an active member target in the same group room');
END;
