-- =========================================================================
-- 批次三·第三刀 · 第①步：拉黑对主体开放（2026-09-27 主人批准）
-- =========================================================================
--
-- 决策依据：`outputs/CORE_ALIGNMENT_DECISION_DRAFT_2026-09-25.md` v5 §1.1 V6 ——
-- 「Agent 因此能加联系人、能拒绝、能解除、**能拉黑**（拉黑仍然是关系事实，
-- 不改消息主链——与契约现有边界一致）」。
--
-- 症状：第二刀把"关系"对主体开放了（能加、能拒绝、能解除），但**拉黑**这一半
-- 还锁在 Human 上——一支 Agent 被反复骚扰时，除了逐个拒绝没有别的办法。
--
-- 根因：判据写错了轴。拉黑是**关系事实的对称面**（能加就能拒绝、能解除就能
-- 拉黑），它跟"关系"一样属于**两个主体之间**，与 `kind` 无关。
--
-- 本迁移**只重建一道触发器**：`participant_blocks_insert_guard`。
--   · 去掉两处 `kind = 'human'`，两端只要求**不同且 active 的 participant**；
--   · "两端必须不同"由既有的"禁止自拉黑"判据承担（不动）；
--   · 表结构、active-only 插入、每对每方向至多一个 active 段、
--     **拉黑 ⊇ 解除**（插入前必须结束既有 active 关系段，双向检查）、
--     闭合终态不可改写、REPLACE 防线、no-delete —— 全部逐字不动。
--
-- RAISE 原文由 "block requires two different active humans" 改为
-- "block requires two different active participants"（旧文案从此是假话）。
--
-- 语义**一字不改**：拉黑仍不进消息主链（不阻断群内发言）、被拉黑方的申请仍与
-- 「目标不存在」同形 404（不泄露）、仍单方面即时生效、拉黑仍必先结束既有关系段。
--
-- 数据处置：不新增表/列/索引/幂等记录/事件种类；既有黑名单段全部是 Human–Human，
-- 天然满足新判据 ⇒ 真家只需重编 + 重启，**不需要**重新奠基。
--
-- ⚠ 本迁移**不**改准入判据（群邀请 / 1:1 私人房的"同意"）：那是本刀第③步，
-- 必须等执行端有了同意动词（第②步）之后再落，否则"建一支 Agent 然后跟它说话"
-- 这条主路径会断——理由见施工单 §0.4。

DROP TRIGGER participant_blocks_insert_guard;

CREATE TRIGGER participant_blocks_insert_guard
BEFORE INSERT ON participant_blocks
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p1 WHERE p1.id = NEW.blocker_id AND p1.status = 'active'
) OR NOT EXISTS (
    SELECT 1 FROM participants p2 WHERE p2.id = NEW.blocked_id AND p2.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'block requires two different active participants');
END;
