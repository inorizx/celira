-- =========================================================================
-- M1 补账：给既有的 active 成员周期补上读游标行（2026-09-27 真机验出）
-- =========================================================================
--
-- 症状：M1 的 `read_cursors_membership_insert` 只在**建成员周期**时创建 cursor
-- 行（`ALTER`/`RENAME` 不会给既有行补）。于是**迁移之前就存在**的周期没有行；
-- 而推进的见证守卫（`read_cursor_advancements_insert_guard`，migration
-- 202608120002）要求「存在一行 cursor 且 read_sequence = from_sequence」
-- ⇒ 那些周期**推不动**。
--
-- 真机证据（2026-09-27，D:\celira-home）：
--   · 5 条活跃 Agent 周期，**5 条都没有 cursor 行**；
--   · 在真库副本上直接 INSERT 一条指向其中一条周期的 advancement：
--       ABORT: read cursor advancement must move from the current cursor
--              to a visible same-room message
--   · 单测 / HTTP 用例 / 独立进程黑盒**都没抓到**——它们的成员周期全是迁移
--     **之后**建的，触发器总会建行；"既有行"这条路径从没被走过
--     （见 outputs/CORE_ALIGN_KNIFE_04_HOME_DRILL_REPORT.md）。
--
-- 修法（第一性原理）：不变式是「**每个成员周期有一条读游标**」——行的权威
-- 创建者是建周期那条触发器，因此**不给它加第二条创建路径**，只**一次性补齐
-- 历史**：给每个 active 周期补一行 join boundary（read_sequence =
-- join_after_sequence、last_read_message_id = NULL、created_at = 该周期的
-- joined_at——"这行属于那个周期，那时建的"）。
--
-- 边界（如实）：
-- · 只补 **active** 周期。closed 周期的游标不参与任何推进（推进守卫要求周期
--   active），关闭时游标已冻结；给它们补行只是没有消费者的数据。
-- · **排除群主转让打开的两种周期**：它们的合法来路是「继承前序 cursor」（同一
--   事务内由 transition 精确见证），**不是**"新边界"。真要缺行，那是另一种异常，
--   不该被这里悄悄改成"从边界开始"——下面那句与建行守卫的排除项**同形**，
--   因此这类行会走守卫的继承分支或被守卫拒绝，不会被悄悄改写。
-- · 补的行走的还是**同一个建行守卫**的新边界分支（不绕过它）：身份不对
--   （membership / participant / room 不匹配）会在**迁移时当场 abort**，
--   而不是留到运行时才发现。

INSERT INTO read_cursors (
    membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at)
SELECT m.membership_id, m.participant_id, m.room_id, m.join_after_sequence, NULL, m.joined_at
FROM room_memberships m
WHERE m.active_marker = 1
  AND NOT EXISTS (SELECT 1 FROM read_cursors c WHERE c.membership_id = m.membership_id)
  AND NOT EXISTS (
      SELECT 1 FROM group_owner_transitions t
      WHERE t.room_id = m.room_id
        AND (t.new_owner_membership_id = m.membership_id
             OR t.old_owner_new_member_membership_id = m.membership_id)
  );
