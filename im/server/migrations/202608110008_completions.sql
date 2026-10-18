-- Celira Core V1：Completion 与 Agent Message Writeback 切片。
--
-- 契约：CORE_V1_COMPLETIONS.md
-- - completions 是 Backend 对某个 Agent Event 提交、并由 Core 持久化的
--   处理结果事实：只描述处理终态（reply / no_reply / failure），不复制
--   Event 正文，不承载模型、Prompt、Memory、Tool、Session 或 Run；
-- - Completion 持久身份是 (event_id, target_agent_id)（复合主键），不另设
--   completion_id 或调用方可选的 Completion-Key；target_agent_id 必须等于
--   Event 的目标 Agent（INSERT 守卫拒绝错误 target）；
-- - outcome 只允许 reply / no_reply / failure；reply 必须携带非空
--   reply_message_id，no_reply/failure 必须为空（CHECK 一致性）；未知
--   outcome、NUL/BLOB 存储类一律拒绝；
-- - reply_message_id 必须指向 source Event 所在 Room 中、actor 为 target
--   Agent 的普通 Message（INSERT 守卫）；reply 的正文与幂等 key 仍由
--   messages 唯一事实保存，completions 不建立第二份正文或第二份 Message
--   幂等事实；
-- - Completion 一经提交全字段不可 UPDATE（immutable 触发器）、不可 DELETE
--   （no-delete 触发器）；
-- - 复合身份 (event_id, target_agent_id) 已存在时拒绝任何新插入（BEFORE
--   INSERT 守卫）：普通重复 INSERT、INSERT OR REPLACE / REPLACE 都不能绕过
--   immutable / no-delete 触发器改写或重建既有 Completion；
-- - 本迁移不新增 ACK、Cursor、Replay、attempt、Lease、Runtime、Provider
--   或 Agent 私有表；migration 001–007 不修改，旧库升级不回填 Completion。

CREATE TABLE completions (
    event_id         TEXT NOT NULL REFERENCES agent_events(id),
    target_agent_id  TEXT NOT NULL REFERENCES participants(id),
    outcome          TEXT NOT NULL,
    reply_message_id TEXT REFERENCES messages(id),
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (event_id, target_agent_id),
    CHECK (typeof(event_id) = 'text'),
    CHECK (typeof(target_agent_id) = 'text'),
    CHECK (typeof(outcome) = 'text'),
    CHECK (typeof(reply_message_id) IN ('text', 'null')),
    CHECK (typeof(created_at) = 'integer'),
    CHECK (instr(event_id, char(0)) = 0),
    CHECK (instr(target_agent_id, char(0)) = 0),
    CHECK (instr(outcome, char(0)) = 0),
    CHECK (reply_message_id IS NULL OR instr(reply_message_id, char(0)) = 0),
    CHECK (outcome IN ('reply', 'no_reply', 'failure')),
    CHECK (
        (outcome = 'reply' AND reply_message_id IS NOT NULL)
        OR (outcome <> 'reply' AND reply_message_id IS NULL)
    )
);

-- 写入守卫：Event 必须存在、target 必须等于 Event 的目标 Agent；outcome
-- 为 reply 时 reply_message_id 必须指向 source Event 所在 Room 中 actor 为
-- target Agent 的普通 Message（错误 target、错误 Message 关联一律拒绝）。
-- 本守卫与表级 CHECK 各自独立；同一写入同时违反多个约束时，具体先返回
-- 哪条数据库错误不属于契约，正确性不得依赖同类 BEFORE trigger 的执行顺序。
CREATE TRIGGER completions_insert_guard
BEFORE INSERT ON completions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM agent_events e
    WHERE e.id = NEW.event_id
      AND e.target_agent_id = NEW.target_agent_id
      AND (
          NEW.outcome <> 'reply'
          OR (
              NEW.reply_message_id IS NOT NULL
              AND EXISTS (
                  SELECT 1
                  FROM messages m
                  WHERE m.id = NEW.reply_message_id
                    AND m.actor_id = NEW.target_agent_id
                    AND m.room_id = (
                        SELECT room_id FROM messages WHERE id = e.message_id
                    )
              )
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'completion requires the event target match and a reply message from the target agent in the event room');
END;

-- 复合身份防改写：INSERT OR REPLACE / REPLACE 在 (event_id, target_agent_id)
-- 主键冲突时先删除旧行再插入新行，可绕过 immutable / no-delete 触发器重写
-- 既有 Completion；本守卫在 INSERT 阶段直接拒绝任何复合身份已存在的写入
-- （普通重复 INSERT 也由本守卫或主键唯一约束拒绝）。本守卫与
-- completions_insert_guard 相互独立，同一写入同时违反多个约束时先返回哪条
-- 数据库错误不属于契约。
CREATE TRIGGER completions_insert_identity_guard
BEFORE INSERT ON completions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM completions
    WHERE event_id = NEW.event_id
      AND target_agent_id = NEW.target_agent_id
)
BEGIN
    SELECT RAISE(ABORT, 'completion identity (event_id, target_agent_id) already exists');
END;

-- Completion 事实一经提交不可改写（等值自赋值不触发）。
CREATE TRIGGER completions_immutable
BEFORE UPDATE ON completions
FOR EACH ROW
WHEN NEW.event_id IS NOT OLD.event_id
  OR NEW.target_agent_id IS NOT OLD.target_agent_id
  OR NEW.outcome IS NOT OLD.outcome
  OR NEW.reply_message_id IS NOT OLD.reply_message_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'completions are immutable');
END;

-- Completion 不允许硬删除。
CREATE TRIGGER completions_no_delete
BEFORE DELETE ON completions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'completions cannot be deleted');
END;
