-- =========================================================================
-- 批次四·表达权 第一刀：Agent 写自己的资料（2026-09-28 主人方向「表达权这种」）
-- =========================================================================
--
-- 症状：一支 Agent 不能写自己的资料——它对自己的介绍**只能由 Owner 代笔**。
-- 根因：写路径的判据写的是"谁拥有它"（ownership），而不是"这是谁的资料"。
--      与批次三同一条轴：**归属不是代笔权**。
--
-- 本迁移**只重建三道触发器**（不动表、不搬行、不新增持久对象、不新增列；
-- ⇒ 真家只需重编 + 重启，**不需要**重新奠基）：
--
--   ① `participant_profile_versions_published_by_human`
--      发布者由「active Human」放开为「active Human **或该 Agent 自己**」。
--   ② `participant_profile_versions_target_owned_agent`
--      目标由「被发布者拥有的 active Agent」放开为「被发布者拥有的 active Agent
--      **或发布者自己**」。
--   ③ `participant_profile_put_idempotency_actor_human`
--      幂等记录 actor 与 ① 同形（active Human 或该 Agent 自己）。
--
-- 形状与批次三第③步**同一条**：**并集**（主人裁决「像飞书 QQ 那种，人类用户把它
-- 带进来」）——Owner 的写权限**一字不动**，只是多了"它自己"这一支。
--
-- `published_by_id` 本来就是 ParticipantId（记"谁发布的"），所以 Agent 自己发布
-- **不需要新列、不需要新表**。
--
-- 其余判据逐字不动：版本严格 +1、整行不可改写、no-delete、身份守卫、
-- 文本卫生（NUL / 字节上限 / 空串规范化）、幂等记录不可改写、见证一致性。
--
-- 本迁移**不**放开：房呈现（Group Room Presentation，房治理面，actor 仍限该房
-- active owner）、头像上传（multipart 另一条通道）、Publication / Service 层。

-- ① 发布者：active Human 或该 Agent 自己。
DROP TRIGGER participant_profile_versions_published_by_human;

CREATE TRIGGER participant_profile_versions_published_by_human
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.published_by_id
      AND p.status = 'active'
      AND (p.kind = 'human' OR p.id = NEW.participant_id)
)
BEGIN
    SELECT RAISE(ABORT, 'participant profile publisher must be an active human or the agent itself');
END;

-- ② 目标：被发布者拥有的 active Agent，或发布者自己。
DROP TRIGGER participant_profile_versions_target_owned_agent;

CREATE TRIGGER participant_profile_versions_target_owned_agent
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM participants p
    WHERE p.id = NEW.participant_id
      AND p.kind = 'agent'
      AND p.status = 'active'
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
BEGIN
    SELECT RAISE(ABORT, 'participant profile requires an active agent owned by the publisher or the agent itself');
END;

-- ③ 幂等记录 actor：与 ① 同形。
DROP TRIGGER participant_profile_put_idempotency_actor_human;

CREATE TRIGGER participant_profile_put_idempotency_actor_human
BEFORE INSERT ON participant_profile_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_human_id
      AND p.status = 'active'
      AND (p.kind = 'human' OR p.id = NEW.participant_id)
)
BEGIN
    SELECT RAISE(ABORT, 'profile idempotency actor must be an active human or the agent itself');
END;
