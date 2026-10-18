-- Celira Core V1：Agent Delivery Position / ACK 切片。
--
-- 契约：CORE_V1_AGENT_ACK.md
-- - agent_delivery_positions 是每 Agent 唯一、单调不回退的耐久投递位置：
--   一行 = 该 Agent 已 ACK 到的最后一个 Event（event_id + 其 sequence 的
--   内部副本）；位置只归 target Agent，不归 Binding、Credential 或进程
--   实例；Credential 轮换、Binding 间隙、Core 重启都不重置位置；
-- - 位置行引用该 Agent 自己的真实 Event：INSERT/UPDATE 守卫要求
--   event_id 存在、target_agent_id = 该 Event 的目标 Agent、
--   sequence = 该 Event 的真实 sequence（防伪造引用与伪造边界）；
-- - 只能单调向前：UPDATE 守卫拒绝回退、相等推进与 agent_id/created_at
--   改写；INSERT 只允许该 Agent 首次（不存在既有行），INSERT OR REPLACE /
--   REPLACE 不能绕过守卫重建或改写既有位置；DELETE 一律拒绝；
-- - 连续定稿前缀守卫（数据库层独立兜底，谓词与应用层前缀校验一致）：
--   INSERT 时该 Agent [1, NEW.sequence] 内每个 Event、UPDATE 时
--   (OLD.sequence, NEW.sequence] 内每个 Event 都必须已定稿——定稿 =
--   kind 是 message.recalled（更正通知，不产生 Completion）、或 source
--   Message 已有 message_recalls 见证、或已存在匹配 (event_id,
--   target_agent_id) 的持久 Completion（显式同时匹配两列，不靠同一
--   Agent 内隐含成立）；存在任一未定稿 Event 即 RAISE(ABORT)，事务不落
--   位置：直接 SQL 伪造前进或跳过未定稿前缀同样被数据库拒绝，任何写入
--   权威数据库的路径都不得制造「ACK 已越过未处理 Event」的假事实；
-- - 与 ACK 事务同一 BEGIN IMMEDIATE 线性化边界：并发 ACK 最终收敛到
--   唯一合法位置，不出现半态；失败与取消不得推进位置；
-- - Agent revoke 后位置行保留（耐久协调/审计事实），但 Backend 认证
--   fail closed，不再可经公开协议观察或推进（无撤销传播触发器）；
-- - Event 行与 Completion 行继续不可修改/删除；投递位置绝不塞回 Event
--   或 Completion；本迁移不引入 attempt、Lease、Runtime 或 Agent 私有表。

CREATE TABLE agent_delivery_positions (
    agent_id   TEXT PRIMARY KEY REFERENCES participants(id),
    event_id   TEXT NOT NULL REFERENCES agent_events(id),
    sequence   INTEGER NOT NULL CHECK (sequence > 0),
    created_at INTEGER NOT NULL,
    CHECK (typeof(agent_id) = 'text'),
    CHECK (typeof(event_id) = 'text'),
    CHECK (typeof(sequence) = 'integer'),
    CHECK (typeof(created_at) = 'integer'),
    CHECK (instr(agent_id, char(0)) = 0),
    CHECK (instr(event_id, char(0)) = 0)
);

-- 写入守卫：位置必须引用该 Agent 自己的真实 Event，且 sequence 必须
-- 精确等于该 Event 的 sequence（伪造引用、跨 Agent 引用、伪造边界一律
-- 拒绝）。本守卫与 identity / forward 守卫彼此独立。
CREATE TRIGGER agent_delivery_positions_write_guard
BEFORE INSERT ON agent_delivery_positions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM agent_events e
    WHERE e.id = NEW.event_id
      AND e.target_agent_id = NEW.agent_id
      AND e.sequence = NEW.sequence
)
BEGIN
    SELECT RAISE(ABORT, 'delivery position must reference the agent''s own event with its real sequence');
END;

-- 每 Agent 至多一行：既有行存在时拒绝任何新插入（普通重复 INSERT、
-- INSERT OR REPLACE / REPLACE 都不能绕过 no-delete / forward 守卫重建或
-- 改写既有位置）。
CREATE TRIGGER agent_delivery_positions_insert_identity_guard
BEFORE INSERT ON agent_delivery_positions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM agent_delivery_positions WHERE agent_id = NEW.agent_id
)
BEGIN
    SELECT RAISE(ABORT, 'delivery position already exists for the agent');
END;

-- 位置不可回退：agent_id 与 created_at 不可改写，sequence 只能严格向前
-- （相等推进与回退一律拒绝，由 ACK 用例把同/更早位置裁决为幂等 200 无
-- 副作用，不落库）。
CREATE TRIGGER agent_delivery_positions_forward_guard
BEFORE UPDATE ON agent_delivery_positions
FOR EACH ROW
WHEN NEW.agent_id IS NOT OLD.agent_id
  OR NEW.sequence <= OLD.sequence
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'delivery position must only advance strictly forward');
END;

-- 连续定稿前缀守卫（INSERT）：首次落位置时，该 Agent 从 sequence 1 到
-- NEW.sequence 的每个 Event 都必须已定稿。定稿 = kind 是
-- message.recalled（更正通知，不产生 Completion）、或 source Message
-- 已有 message_recalls 见证（首次 Completion 按既有契约不再允许）、或
-- 已存在匹配 (event_id, target_agent_id) 的持久 Completion（不靠「同一
-- Agent 内必有一致 target」的隐含假设，显式同时匹配两列）。存在任一未
-- 定稿 Event → 整个语句/事务 ABORT，位置不落库：真实引用 + 严格前进
-- 不足以构成合法位置，直接 SQL 跳号或跳过未定稿前缀同样被拒。
-- 本守卫与 write / identity / forward 守卫彼此独立，同一写入同时违反
-- 多个守卫时先返回哪条数据库错误不属于契约。
CREATE TRIGGER agent_delivery_positions_insert_finalized_prefix_guard
BEFORE INSERT ON agent_delivery_positions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM agent_events e
    WHERE e.target_agent_id = NEW.agent_id
      AND e.sequence <= NEW.sequence
      AND e.kind <> 'message.recalled'
      AND NOT EXISTS (
          SELECT 1 FROM message_recalls r WHERE r.message_id = e.message_id
      )
      AND NOT EXISTS (
          SELECT 1 FROM completions c
          WHERE c.event_id = e.id AND c.target_agent_id = e.target_agent_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'delivery position insert requires every event through the target to be finalized');
END;

-- 连续定稿前缀守卫（UPDATE）：推进时 (OLD.sequence, NEW.sequence] 内
-- 每个 Event 都必须已定稿（定稿谓词与上方 INSERT 守卫完全相同）。跨
-- 未定稿 Event 跳到更后真实 Event、或跳到未定稿目标本身，一律
-- RAISE(ABORT) 位置不动；前进范围为空（回退/相等）时由 forward 守卫
-- 拒绝。本守卫与 write / forward / update 守卫彼此独立，同一写入同时
-- 违反多个守卫时先返回哪条数据库错误不属于契约。
CREATE TRIGGER agent_delivery_positions_update_finalized_prefix_guard
BEFORE UPDATE ON agent_delivery_positions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM agent_events e
    WHERE e.target_agent_id = NEW.agent_id
      AND e.sequence > OLD.sequence
      AND e.sequence <= NEW.sequence
      AND e.kind <> 'message.recalled'
      AND NOT EXISTS (
          SELECT 1 FROM message_recalls r WHERE r.message_id = e.message_id
      )
      AND NOT EXISTS (
          SELECT 1 FROM completions c
          WHERE c.event_id = e.id AND c.target_agent_id = e.target_agent_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'delivery position update requires every event in the advanced range to be finalized');
END;

-- 更新同样必须引用该 Agent 自己的真实 Event（配合 forward 守卫）。
CREATE TRIGGER agent_delivery_positions_update_guard
BEFORE UPDATE ON agent_delivery_positions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM agent_events e
    WHERE e.id = NEW.event_id
      AND e.target_agent_id = NEW.agent_id
      AND e.sequence = NEW.sequence
)
BEGIN
    SELECT RAISE(ABORT, 'delivery position update must reference the agent''s own event with its real sequence');
END;

-- 位置不允许硬删除。
CREATE TRIGGER agent_delivery_positions_no_delete
BEFORE DELETE ON agent_delivery_positions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'delivery positions cannot be deleted');
END;
