-- =========================================================================
-- 表达权第四刀：**人写自己的头像**（2026-10-17 主人批准）
-- =========================================================================
--
-- 现场：主人问「用户作为 IM 的用户，肯定是要有自己的头像名字什么的吧，这种应该是基于
-- im core 的吧？」，随后从第一性原理定了归属，并批准本切片
-- （裁决材料：`outputs/HUMAN_PROFILE_RULING_DRAFT_2026-10-17.md`）。
--
-- 归属结论（三问）：换成微信/QQ 还成立吗 ⇒ 名字/头像**归世界**；别人要不要看见 ⇒
-- 世界必须持有一份权威；是不是"谁说的" ⇒ 归**主体**且必须署名。
-- ⇒ 客户端只放凭据与缓存（不许存一份当权威）；id 由世界颁发；换世界不迁移。
--
-- 症状：**人自己**的名字/头像在世界上没有写入口 —— 名字只有兑换那一次
-- （`new_human` 时 `display_name` 必填），头像则连通道都没有。
--
-- 根因：写路径的判据写的是「目标是 agent」——它问的是"这是不是一支 Agent"，
--      而不是"这是不是**你自己的**资料"。与批次四前三刀同一条轴：**归属不是代笔权**。
--
-- 本迁移**只重建一道触发器**（不动表、不搬行、不新增列/表/幂等记录；
-- ⇒ 真家只需重编 + 重启，**不需要**重新奠基）：
--
--   `participant_profile_versions_target_owned_agent`
--     目标由「被发布者拥有的 active Agent，或发布者自己」放开为
--     「……**或**（active Human 且发布者就是它自己）」。
--
-- ⚠ 另外两道守卫**不用改**，如实记在这里免得下次有人以为漏了：
--   · `participant_profile_versions_published_by_human` 本来就是「active Human 或该目标自己」⇒ 人写自己已通过；
--   · `participant_profile_put_idempotency_actor_human` 与它同形 ⇒ 同样已通过。
--
-- ⚠ **名字不在这里**：`display_name` 是**既有 Participant 身份事实**
-- （呈现切片明文「不复制 `display_name` / `status` / `kind`」），它住在 `participants` 表，
-- 而那张表的不可改写守卫只管 `kind` / `status` / `revoked_at`
-- （`202608110003_agent_identity.sql`）⇒ **`display_name` 本来就可写**，且表上已有
-- ≤80 / 禁 NUL / trim 非空三条 CHECK。所以名字那一半是**状态写**（无版本行、无幂等键，
-- 照 `PUT /v1/backend/rooms/{id}/read-cursor` 的先例），**不需要任何 migration** ——
-- 本迁移只管头像。
--
-- 人**只能写自己**：没有 Owner 概念（Human 不被谁拥有），所以人类这一支不带任何
-- "替别人写"的分支。Agent 那一支（active Owner 或该 Agent 自己）**逐字保留**。
--
-- 其余判据逐字不动：版本严格 +1、整行不可改写、no-delete、身份守卫、文本卫生、
-- 幂等记录不可改写、见证一致性。

DROP TRIGGER participant_profile_versions_target_owned_agent;

CREATE TRIGGER participant_profile_versions_target_owned_agent
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM participants p
    WHERE p.id = NEW.participant_id
      AND p.status = 'active'
      AND (
        -- ① Agent：发布者是它自己，或是它的 active Owner（批次四第一刀，逐字保留）。
        (
          p.kind = 'agent'
          AND (
            NEW.published_by_id = NEW.participant_id
            OR EXISTS (
                SELECT 1 FROM agent_owners ao
                WHERE ao.agent_id = p.id
                  AND ao.human_id = NEW.published_by_id
                  AND ao.status = 'active'
            )
          )
        )
        -- ② Human：**只能写自己**（表达权第四刀，2026-10-17）。没有 Owner 这一支。
        OR (p.kind = 'human' AND NEW.published_by_id = NEW.participant_id)
      )
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile requires an active agent owned by the publisher, the agent itself, or the human itself');
END;
