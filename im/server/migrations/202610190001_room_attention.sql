-- Celira Core V1：提醒档（attention；2026-10-19 窄解冻切片，1a）。
--
-- 契约：CORE_V1_IM_CONTRACT.md §9.1（事实与写读面）、
--       CORE_V1_AGENT_DIRECTORY.md §3.2（BackendRoomView 的 attention 一格）。
--
-- 一张表 + 两道守卫。**schema-only、不搬行、不预建行**：既有库里一行都没
-- 有这条新表的数据要造，也**不需要**造——「缺行 = all」是投影的默认值，
-- 不是一条待补齐的缺失事实（与 §9 读游标那族「缺行＝按 join boundary
-- 兜底」同一手法，区别在于这里**没有**兜底取值需要落库：all 就是默认）。
--
-- ── 这张表是什么 ─────────────────────────────────────────────────────
--
-- 群房里「这间房的动静要不要提醒我」是**个人事实**：每 (Room × Participant)
-- 至多一行，level ∈ {all, mentions}。它与 §9 的读位置同一档——
--   · **谁写**：只有本人（身份只来自认证，请求体里没有 participant_id）；
--   · **谁看得见**：只有本人（两条列表各自的投影，不构成对别人的可观察面）；
--   · **kind 不参与判据**：人对它、agent 对它，规则一字不差。
--
-- ⚠ **它不改消息主链**：消息照常提交、未读照常计算、历史照常可读。这一档
-- 只决定那条**易失**通知发不发（Agent 侧是门铃 `events.available`，Human
-- 侧仍是 WS 提示）⇒ **任何一档都不丢消息**。
--
-- ⚠ **本迁移只落事实与读面**。「按档决定发不发那条易失通知」是另一刀
-- （1b）——它是本切片里唯一改变「谁被叫醒」的改动，落之前要先量 ping
-- 密度。**本迁移不碰 messages / agent_events / human_events /
-- human_event_deliveries / read_cursors 的任何一行与任何触发器**，
-- 也不新增任何事件 kind。
--
-- ── 两道守卫（fail closed，直接 SQL 也绕不过）─────────────────────────
--
-- 1. `room_attention_insert_guard`：**只 group 房** + **本人是该房 active 成员**。
--    两人房（`direct` / `human_direct`）里没有「群消息」这一档 ⇒ 写不进来；
--    不在房里（或已离群，周期已 closed）⇒ 写不进来。「房间不存在 / 不是群 /
--    我不是成员」在同一处被拒，对外由应用层统一收成 404 room_not_found，
--    与非成员同形——**不泄露房间是否存在**。
--
-- 2. `room_attention_update_guard`：行身份 `(room_id, participant_id)` **不可
--    改写** ⇒ **只能写本人那一行**（把一个已存在的行"转投"到别人名下、或
--    搬到另一间房，都被拒绝）；改值同样必须仍满足「group 房 + active 成员」
--    （离群之后既不能新建、也不能改）。
--    ⚠ 这一道守到的只是**行的身份**；"写的是不是本人"那一半在数据库里没有
--    主语可判——它由写路径保证：`PUT` 的 `participant_id` 只来自认证上下文，
--    请求体里**只有 `level`**（`deny_unknown_fields`）。这与 §9 读游标同形：
--    数据库守卫管形状，身份管在边界。
--
-- level 的闭集由表级 CHECK 裁决（INSERT 与 UPDATE 都受它约束），因此
-- 「取值不在 {all, mentions}」不会漏过任何一条写路径。
--
-- 行可删：删掉一行**恰好等于** all（缺行 = all），没有第二条语义，因此本
-- 迁移不设 no-delete。API 面也没有删除动词——改回 all 就是写 all。

CREATE TABLE room_attention (
    room_id        TEXT NOT NULL REFERENCES rooms(id),
    participant_id TEXT NOT NULL REFERENCES participants(id),
    level          TEXT NOT NULL CHECK (level IN ('all', 'mentions')),
    -- 最近一次设定时刻（状态写，不是不可变事实）。
    updated_at     INTEGER NOT NULL,
    -- 每 (Room × Participant) 至多一行：主键就是身份，不存在第二行。
    PRIMARY KEY (room_id, participant_id),
    CHECK (typeof(room_id) = 'text'),
    CHECK (typeof(participant_id) = 'text'),
    CHECK (typeof(level) = 'text'),
    CHECK (typeof(updated_at) = 'integer')
);

-- 列表投影按 (room, participant) 逐行 LEFT JOIN 取；主键的前缀
-- (room_id, *) 已由主键索引覆盖，这里补 participant 一侧（"我在哪些房间
-- 设过档"，量级与成员周期同阶）。
CREATE INDEX idx_room_attention_participant ON room_attention(participant_id);

-- ── 守卫 1：只 group 房 + 本人是该房 active 成员 ─────────────────────────
-- 直接 SQL 往 direct / human_direct 房插、往自己不在（或已离群）的房里插，
-- 全部在此被拒。
CREATE TRIGGER room_attention_insert_guard
BEFORE INSERT ON room_attention
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM rooms r
        WHERE r.id = NEW.room_id AND r.kind = 'group'
    )
    AND EXISTS (
        SELECT 1 FROM room_memberships m
        WHERE m.room_id = NEW.room_id
          AND m.participant_id = NEW.participant_id
          AND m.active_marker = 1
    )
)
BEGIN
    SELECT RAISE(ABORT, 'room attention requires a group room and an active membership of that participant');
END;

-- ── 守卫 2：只能写本人那一行（行身份不可改写）+ 改值仍须 group + active ───
-- 允许的改写形状只有一种：**行身份原样不动 + 该房仍是 group + 该主体仍是
-- active 成员**。三条里任一不成立即拒——
--   · 身份两条挡住「把别人的行转到我名下」与「把我的行转投给别人 / 搬到另一
--     间房」⇒ **只能写本人那一行**；
--   · 后两条是写入时的判据，不是历史判据（离群之后这一档冻结在离群前的取值）。
CREATE TRIGGER room_attention_update_guard
BEFORE UPDATE ON room_attention
FOR EACH ROW
WHEN NOT (
    NEW.room_id = OLD.room_id
    AND NEW.participant_id = OLD.participant_id
    AND EXISTS (
        SELECT 1 FROM rooms r
        WHERE r.id = NEW.room_id AND r.kind = 'group'
    )
    AND EXISTS (
        SELECT 1 FROM room_memberships m
        WHERE m.room_id = NEW.room_id
          AND m.participant_id = NEW.participant_id
          AND m.active_marker = 1
    )
)
BEGIN
    SELECT RAISE(ABORT, 'room attention rows can only be rewritten in place by an active member of a group room');
END;
