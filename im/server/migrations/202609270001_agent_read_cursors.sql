-- Celira Core V1：Agent 读游标（窄解冻切片 M1）。
--
-- 契约：CORE_V1_AGENT_READ_CURSOR.md（本切片）与 CORE_V1_IM_CONTRACT.md §9
--       （read state 的既有语义——本迁移把同一套交给 Agent，不造第二套）。
--
-- 这一迁移只做三件事，别的一律不动：
--
--   1. **身份列改名**：read_cursors.human_id / read_cursor_advancements.human_id
--      → participant_id。名字是事实的一部分：Agent 的游标不该住在一个叫
--      human_id 的列里。用 `ALTER TABLE … RENAME COLUMN`（不是重建表）——
--      数据保留、既有行一个字节不动；引用这两张表的**其它**触发器与索引
--      由 SQLite 自动改写（本机 3.45 实测：触发器正文与索引定义都会跟着
--      改），因此下面只重建**语义要变**的那三道。
--
--   2. **放开两道守卫**，让 Agent Membership 周期也有一条 cursor：
--      · `read_cursors_insert_guard`：去掉 `p.kind = 'human'` 那道判据；
--      · `read_cursors_membership_insert`：去掉 WHEN 的 kind 限定。
--      其余判据（同房同成员、成员窗口、active、单调、精确见证、转让继承）
--      **一字不动**——本切片只开门，不改门。
--
--   3. **事件面加闸**：`human_event_facts_read_cursor_advanced` 加 WHEN，
--      **只对 Human 的 advancement 造 fact** ⇒ Agent 的推进不产生任何事件。
--      理由（施工单 §0.2 已定默认）：Human Event 的语义是"给 Human 的事件"，
--      而 read.cursor.advanced 按契约只投递**推进者本人**（IM_CONTRACT §10.1）；
--      Agent 只有一个执行端、位置本来就在读面上读得到，事件对它没有收件人。
--      ——「对方知道我读了」这件事连 Human 都还没有，不在地基刀里发明。
--
-- 不新增表、不新增列、不改任何既有行、不改任何既有端点语义或响应形状。
-- Agent 周期不产生转让继承（transfer 只对 Human 成员），故 (b) 分支自然不匹配。

-- ── 1. 身份列改名 ────────────────────────────────────────────────────────
ALTER TABLE read_cursors RENAME COLUMN human_id TO participant_id;
ALTER TABLE read_cursor_advancements RENAME COLUMN human_id TO participant_id;

-- 索引名跟着事实走（定义由 RENAME COLUMN 自动改写，名字不会）。
DROP INDEX idx_read_cursors_human;
CREATE INDEX idx_read_cursors_participant ON read_cursors(participant_id);

DROP INDEX idx_read_cursor_advancements_human;
CREATE INDEX idx_read_cursor_advancements_participant
    ON read_cursor_advancements(participant_id);

DROP TRIGGER read_cursors_insert_guard;

-- ── 2a. cursor 建行守卫（重建自 202608120002；放开 kind 判据）─────────────
-- 合法来路仍是两种：(a) 新/重入/奠基周期的 join boundary；(b) 群主转让
-- 打开的新 owner / 旧 owner member 周期继承前序 cursor（由同一事务内的
-- transition 精确见证）。原来第一段 EXISTS 用 `p.kind = 'human'` 排掉
-- Agent 周期；现在只要求"cursor 的身份列 = 该周期的 participant 且同房"
-- —— 这一句对两种主体是同一条判据。
CREATE TRIGGER read_cursors_insert_guard
BEFORE INSERT ON read_cursors
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM room_memberships mm
        WHERE mm.membership_id = NEW.membership_id
          AND mm.room_id = NEW.room_id
          AND mm.participant_id = NEW.participant_id
    )
    AND (
        (
            NEW.last_read_message_id IS NULL
            AND NEW.read_sequence = (
                SELECT join_after_sequence FROM room_memberships
                WHERE membership_id = NEW.membership_id
            )
            AND NOT EXISTS (
                SELECT 1 FROM group_owner_transitions t
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
            )
        )
        OR
        (
            NEW.read_sequence = (
                SELECT pc.read_sequence
                FROM group_owner_transitions t
                JOIN room_memberships mm ON mm.membership_id = NEW.membership_id
                JOIN room_memberships pred ON pred.membership_id = CASE
                    WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                    WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
                END
                JOIN read_cursors pc ON pc.membership_id = pred.membership_id
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
                  AND t.occurred_at = mm.joined_at
                  AND t.boundary_sequence = mm.join_after_sequence
            )
            AND NEW.last_read_message_id IS (
                SELECT pc.last_read_message_id
                FROM group_owner_transitions t
                JOIN room_memberships mm ON mm.membership_id = NEW.membership_id
                JOIN room_memberships pred ON pred.membership_id = CASE
                    WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                    WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
                END
                JOIN read_cursors pc ON pc.membership_id = pred.membership_id
                WHERE t.room_id = NEW.room_id
                  AND (t.new_owner_membership_id = NEW.membership_id
                       OR t.old_owner_new_member_membership_id = NEW.membership_id)
                  AND t.occurred_at = mm.joined_at
                  AND t.boundary_sequence = mm.join_after_sequence
            )
        )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'read cursor must be a fresh join boundary or a same-transaction transfer inheritance of the predecessor cursor');
END;

DROP TRIGGER read_cursors_membership_insert;

-- ── 2b. 新周期 → 建 cursor 行（重建自 202609250001；去掉 kind 限定）──────
-- 普通加入/重入/奠基 read_sequence = join_after_sequence、
-- last_read_message_id = NULL；转让打开的新 owner / 旧 owner member 周期
-- 继承前序 cursor（由同一事务内已插入的 transition 精确见证）。
-- 本迁移去掉了 `p.kind = 'human'` 那道 WHEN ⇒ Agent 周期同样得到一条 cursor。
CREATE TRIGGER read_cursors_membership_insert
AFTER INSERT ON room_memberships
FOR EACH ROW
BEGIN
    INSERT INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at)
    SELECT NEW.membership_id,
           NEW.participant_id,
           NEW.room_id,
           COALESCE((
               SELECT pc.read_sequence
               FROM group_owner_transitions t
               JOIN room_memberships pred ON pred.membership_id = CASE
                   WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                   WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
               END
               JOIN read_cursors pc ON pc.membership_id = pred.membership_id
               WHERE t.room_id = NEW.room_id
                 AND (t.new_owner_membership_id = NEW.membership_id
                      OR t.old_owner_new_member_membership_id = NEW.membership_id)
                 AND t.occurred_at = NEW.joined_at
                 AND t.boundary_sequence = NEW.join_after_sequence
           ), NEW.join_after_sequence),
           (
               SELECT pc.last_read_message_id
               FROM group_owner_transitions t
               JOIN room_memberships pred ON pred.membership_id = CASE
                   WHEN t.new_owner_membership_id = NEW.membership_id THEN t.target_member_membership_id
                   WHEN t.old_owner_new_member_membership_id = NEW.membership_id THEN t.old_owner_membership_id
               END
               JOIN read_cursors pc ON pc.membership_id = pred.membership_id
               WHERE t.room_id = NEW.room_id
                 AND (t.new_owner_membership_id = NEW.membership_id
                      OR t.old_owner_new_member_membership_id = NEW.membership_id)
                 AND t.occurred_at = NEW.joined_at
                 AND t.boundary_sequence = NEW.join_after_sequence
           ),
           NEW.joined_at;
END;

-- ── 3. 事件面加闸：只对 Human 的推进造 fact（重建自 202608120002）────────
-- Agent 的 advancement 仍然是一条不可变推进事实、仍然推进 cursor（同一
-- 触发链），只是**不生成 Human Event**：没有收件人，也不该给 Agent 发
-- "Human 事件"。snapshot 形状与投递口径一字不变。
DROP TRIGGER human_event_facts_read_cursor_advanced;

CREATE TRIGGER human_event_facts_read_cursor_advanced
AFTER INSERT ON read_cursor_advancements
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.participant_id AND p.kind = 'human'
)
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'read.cursor.advanced',
        NEW.advancement_id,
        NEW.room_id,
        NEW.created_at,
        json_object(
            'room_id', NEW.room_id,
            'last_read_message_id', NEW.last_read_message_id
        );
END;
