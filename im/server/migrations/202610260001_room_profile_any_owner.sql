-- Celira Core v1.1 附加切片：Group Room Profile（群简介）——群主不限参与者 kind。
--
-- 契约：docs/contracts/core-v1/CORE_V1_ROOM_PROFILE.md
--
-- 症状（拓扑级）：owner 自 2026-09-27 起是**房内的治理角色**（`role='owner'`），
-- 不是参与者种类（`kind`）——一支 Agent 可以建房并成为 owner，也可以持有一间群。
-- 但群简介的写面仍把授权钉在 `kind='human'` 上：store 的授权查询、版本插入守卫、
-- 幂等 actor 守卫三处都写了 kind='human'，幂等表列名也还叫 `actor_human_id`。
-- 结果是一支 Agent 群主**读得到自己房的简介、却永远写不进去**：它的房里少了
-- 一格本该由它维护的公共资料。
--
-- 判据不变，只把 kind 从判据里去掉。写入者必须同时满足：
--   ① Room 是 **active 的 group**（direct / human_direct 没有群简介）；
--   ② 写入者是 **active 主体**（不限 kind）；
--   ③ 写入者是该房**当前 active owner**。
-- owner 转让后旧 owner 的周期已关闭 ⇒ 同一守卫自动改判新 owner，不靠任何缓存。
--
-- 本迁移只做三件事（不搬行、不改其他表/列、不重写历史迁移）：
--   1. `room_profile_put_idempotency.actor_human_id` → `actor_id`（列名与事实对齐；
--      SQLite 的 RENAME COLUMN 会同步改写 PK、外键与触发器正文里的引用）；
--   2. 重建 `room_profile_versions_insert_guards`（去掉 kind='human'）；
--   3. 重建幂等 actor 守卫并改名 `room_profile_put_idempotency_actor_active`。
--
-- 202610250001_room_profile.sql 是已提交的历史事实，一个字都不改。

-- ============================================================
-- 1. 幂等表 actor 列正名
-- ============================================================

ALTER TABLE room_profile_put_idempotency RENAME COLUMN actor_human_id TO actor_id;

-- ============================================================
-- 2. 版本写入资格：active group + active 主体 + 当前 active owner
-- ============================================================

DROP TRIGGER room_profile_versions_insert_guards;

CREATE TRIGGER room_profile_versions_insert_guards
BEFORE INSERT ON room_profile_versions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r
    WHERE r.id = NEW.room_id AND r.kind = 'group' AND r.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.updated_by AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships o
    WHERE o.room_id = NEW.room_id
      AND o.participant_id = NEW.updated_by
      AND o.role = 'owner'
      AND o.active_marker = 1
)
BEGIN
    SELECT RAISE(ABORT, 'room profile requires an active group room and its active owner');
END;

-- ============================================================
-- 3. 幂等 actor 守卫：active 主体（不限 kind）
-- ============================================================

DROP TRIGGER room_profile_put_idempotency_actor_human;

CREATE TRIGGER room_profile_put_idempotency_actor_active
BEFORE INSERT ON room_profile_put_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.actor_id AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'room profile idempotency actor must be an active participant');
END;
