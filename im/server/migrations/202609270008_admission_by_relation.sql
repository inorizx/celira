-- =========================================================================
-- 批次三·第三刀 · 第③步：准入补上"关系"这一支（2026-09-27 主人裁决）
-- =========================================================================
--
-- 主人裁决原文：「大概就是像飞书 QQ 那种，Agent 要进来，肯定是人类用户拉他进来得。」
--
-- ⇒ **Agent 是被人类用户拉进来的**：ownership（谁把它带进来）**本来就是准入资格之一**，
-- 不是"代它答应"。因此本刀**不**把 ownership 拿掉，而是把"关系"这一支**补上**：
--
--     私人房准入 = **active ownership** 或 **accepted 关系**（并集）
--
-- 这与群邀请的既有判据**同形**（批次三第二刀已把群成员资格收成"accepted 关系 或
-- 被 owner 拥有的 active Agent"）——一条判据，两处同形。
--
-- 补上关系这一支的直接收益：**消掉第二刀登记的那个缺口**——
-- 「Human 与"自己并不拥有的 Agent"建立关系后仍没有开房路径」。
-- 今天那种 Agent 有 accepted 关系也开不出房；本迁移之后可以。
--
-- 本迁移**只重建一道触发器**（不动表、不搬行、不新增持久对象 ⇒ 真家只需重编 + 重启）：
--   · `direct_room_pairs_insert_active_owner` 的 private 分支：由「active ownership」
--     改成「active ownership **或** accepted 关系段」；service 分支与其余判据逐字不动。
--   · **群成员资格守卫不动**（第二刀收成的并集本来就是对的形状）。
--
-- ⚠ 与存储层配套（同一刀）：`room_store.rs::create_direct_room` 的准入改成同形并集，
-- 并**把同对既有房的重放提到准入之前**——重放是"已经发生过的事实"，排在准入后面
-- 会让"关系结束 / 归属撤销之后，你们那间房再也回不去"成为静默卡死。

DROP TRIGGER direct_room_pairs_insert_active_owner;

CREATE TRIGGER direct_room_pairs_insert_active_owner
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT (
    (NEW.relation = 'private'
     AND (
       EXISTS (SELECT 1 FROM agent_owners
               WHERE agent_id = NEW.agent_id AND human_id = NEW.human_id
                 AND status = 'active')
       OR
       EXISTS (SELECT 1 FROM contact_periods c
               WHERE c.active_marker = 1 AND c.state = 'accepted'
                 AND ((c.a_id = NEW.human_id AND c.b_id = NEW.agent_id)
                   OR (c.a_id = NEW.agent_id AND c.b_id = NEW.human_id)))
     ))
    OR
    (NEW.relation = 'service'
     AND EXISTS (
         SELECT 1
         FROM service_direct_room_bindings b
         JOIN agent_service_accesses sa ON sa.access_id = b.access_id
         JOIN agent_service_publication_versions pv
              ON pv.agent_id = b.agent_id
             AND pv.publication_version = (
                 SELECT MAX(v2.publication_version)
                 FROM agent_service_publication_versions v2
                 WHERE v2.agent_id = b.agent_id)
         WHERE b.room_id = NEW.room_id
           AND b.access_id = NEW.service_access_id
           AND b.human_id = NEW.human_id
           AND b.agent_id = NEW.agent_id
           AND sa.human_id = NEW.human_id
           AND sa.agent_id = NEW.agent_id
           AND sa.status = 'active'
           AND EXISTS (SELECT 1 FROM participants a
                       WHERE a.id = NEW.agent_id AND a.kind = 'agent' AND a.status = 'active')
           AND pv.service_status = 'accepting'
     ))
)
BEGIN
    SELECT RAISE(ABORT, 'direct room pair must witness an active owner or an accepted contact (or an active service access)');
END;
