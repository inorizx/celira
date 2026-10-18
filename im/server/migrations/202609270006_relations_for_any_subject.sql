-- =========================================================================
-- 批次三·第二刀：关系对主体开放（2026-09-27 主人批准「解冻」）
-- =========================================================================
--
-- 症状：一支 Agent 在世界里没有关系能力——别人加不了它（模型两端钉死 Human），
-- 它也加不了别人、更看不见谁在申请。上一刀刚打开的「Agent 也能当 owner」因此
-- 卡在半路：**Agent owner 邀请不到任何人**，因为它名下永远不可能有 accepted 联系人。
--
-- 根因：判据写错了轴。关系是**两个主体之间**的事实，代码里写成了"两个 Human 之间"
-- 的事实——`kind`（你是谁）被当成了关系资格（你能不能有关系）。
--
-- 本迁移**只重建一道触发器**：`contact_periods_insert_guard`。
--   · 去掉两处 `kind = 'human'`，两端只要求 **active**（不分 kind）；
--   · "两端必须不同"由表 CHECK `a_id < b_id` 承担（不动）；
--   · 方向（requester ∈ {a,b}）、状态机（pending → accepted 唯一迁移）、
--     身份不可改写、闭合元组、每对至多一个 active 段、REPLACE 防改写、
--     no-delete —— 全部在别处，且全部 kind 中性，逐字不动。
--
-- RAISE 原文由 "two different active humans" 改为 "two different active
-- participants"：旧文案从此是假话，改名是**有意**的可读变化（有测试断言它）。
--
-- 数据处置：不新增表/列/索引/幂等记录/事件种类；既有段全部是 Human–Human，
-- 天然满足新判据 ⇒ 真家只需重编 + 重启，**不需要重新奠基**。
--
-- ⚠ 本刀**不**放开的两处（都有明文依据，见施工单 §2）：
--   · `participant_blocks` 仍两端限 Human（主人 2026-09-25 明文裁决）；
--   · Service Access 受让方仍限 Human（那是另一个模型：把"我的 Agent"开放给别人用）。

DROP TRIGGER contact_periods_insert_guard;

CREATE TRIGGER contact_periods_insert_guard
BEFORE INSERT ON contact_periods
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p1 WHERE p1.id = NEW.a_id AND p1.status = 'active'
) OR NOT EXISTS (
    SELECT 1 FROM participants p2 WHERE p2.id = NEW.b_id AND p2.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'contact period requires two different active participants');
END;
