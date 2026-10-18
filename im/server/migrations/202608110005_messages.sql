-- Celira Core V1：普通文本 Message 与 History 切片。
--
-- 契约：CORE_V1_MESSAGES.md + CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md +
-- CORE_V1_REPLIES_AND_QUOTES.md
-- - messages 是单一 Message 事实表，同时持有不公开的 Idempotency-Key
--   协调列，不建立第二张 Message 幂等事实表、request digest 或 Room
--   sequence counter 表；
-- - Message ID 为主键；actor + idempotency key 全局唯一（按 actor
--   Participant 作用域，不按 Room 隔离）；Room + sequence 唯一；
--   (room_id, sequence) 唯一索引直接支撑 keyset pagination；
-- - messages.id 必须是与 agent_events.id / human_event_facts.fact_id 同级
--   的严格规范小写连字符 UUID v4（表级 CHECK 全套：typeof、长度、连字符
--   位置、版本位 4、variant、lower、NUL、字符集、去连字符长度）；应用层
--   MessageId::new_v4 生成正确值，SQLite 兜底拒绝直接 SQL 写入的大写、
--   v1/v5、畸形或 BLOB/NUL 形式——这是 public cursor `c1.m.<message_id>`
--   严格 round-trip 的必要不变量：凡服务端会签发为公共 cursor payload 的
--   稳定 ID，数据库必须保证其能被同一严格解析器接受；
-- - sequence 由写入事务内按该 Room 当前最大 sequence 分配，正整数，
--   只在单个 Room 内有意义；数据库不承诺永无间隙；
-- - body 是原样保存的 UTF-8 文本：存储类必须为 TEXT（`typeof(body) =
--   'text'` 拒绝 BLOB 旁路，阻断无效 UTF-8 绕过持久层），再按
--   `length(CAST(body AS BLOB))` 计算 UTF-8 字节数（不是 `length(body)`
--   的字符数），1-16384 bytes；空、超限、含 U+0000 由 CHECK 兜底拒绝；
--   纯空白非空正文合法；应用校验不能替代持久层兜底；
-- - reply_to_message_id 是 messages 表上一列可空、不可变的公共字段：
--   「这条 Message 回复/引用哪条已提交 Message」的唯一事实直接固化在
--   Message 行上（v1 一条 Message 至多一个 target；target 可被任意多条
--   引用；不复制 target 正文、actor 资料、sequence 或可见性快照），
--   不建立第二张关系表——Message AFTER INSERT 的 Human Event 快照在
--   发生时就能读取同行的 reply_to_message_id 一并固化，读取路径原样
--   透传，不存在第二事实源；
-- - 新 Message 插入时 Room 必须 active、actor Participant 必须 active、
--   actor 在该 Room 的 Membership 必须 active（INSERT 守卫）；守卫只要求
--   active 状态，不把 actor kind 写死为 Human——Human-only 由当前公开
--   HTTP 认证入口保证，未来 Agent 只能复用同一提交内核；
-- - Message 一经提交全部公共字段（id/room_id/sequence/actor_id/body/
--   reply_to_message_id/created_at）不可 UPDATE，Message 不可 DELETE；
--   幂等记录随 Message 行一并不可改写；
-- - REPLACE 防线：SQLite 的 INSERT OR REPLACE / REPLACE 在 id、actor +
--   idempotency key 或 Room + sequence 任一身份冲突时先隐式删除冲突行，
--   默认 recursive_triggers=OFF 下不会触发 no-delete 触发器，可绕过
--   immutable / no-delete 守卫替换既有 Message（连带改写其 Mention 与
--   Event 关联）；BEFORE INSERT identity guard 在插入阶段直接拒绝任一
--   身份已存在的写入（普通重复 INSERT 同样被拒）；应用层幂等重放先查询
--   后插入，不依赖数据库重复插入；
-- - 后续 Participant 或 Membership revoke 不删除、不改写历史 Message
--   （messages 不参与撤销传播触发器）；
-- - idempotency key 复用既有 1-128 可见 ASCII、无逗号、无首尾空白、无
--   NUL 的规则（CHECK 兜底），不通过任何公开响应或 History 返回；
-- - mentions 是显式结构化唤醒事实：Message 的每个 Mention 目标恰好一条
--   （(message_id, target_agent_id) 主键）；Mention 记录与 source Message
--   在同一 SQLite 事务中提交（Message 先插入、Mention 后插入，同一事务
--   原子成功或失败，失败回滚不得留下 Message、Mention、Event 或
--   sequence）；INSERT 守卫要求 source Message 的 Room 必须是 group、目标
--   必须是 active Agent 且在该 Room 有 active Membership（提交事务内仍
--   active）；非 group Room 的 Message 不接受任何 Mention；
-- - 单条 Message 的 Mention 目标上限 64（与 Rust 共享常量
--   `MAX_MENTIONS_PER_MESSAGE` 一致，见 ports/message.rs）：同一 message
--   已有 64 条 Mention 时，第 65 条由 BEFORE INSERT 触发器拒绝；应用层与
--   存储层先于逐目标 SQL 循环拒绝超限请求，本触发器是对直接 SQL 的兜底，
--   把最坏事务工作量锁定，防止 128 KiB 请求被放大为数千次校验 / INSERT /
--   Event 触发阻塞单节点全部写者；
-- - 幂等重放裁决把 Mention 集合纳入同一 Message 事实：同 actor + key +
--   Room + body + 规范排序的 Mention 集合才是重放；只改变 Mention 集合
--   也按冲突拒绝（409 idempotency_conflict）；
-- - Mention 记录一经提交不可 UPDATE、不可 DELETE；
-- - 本迁移不引入 Backend、Credential、Agent Event、Completion、ACK、
--   Lease、Replay、Provider、Memory、Tool、MCP、Runtime 或前端对象。

CREATE TABLE messages (
    id                  TEXT PRIMARY KEY,
    room_id             TEXT NOT NULL REFERENCES rooms(id),
    sequence            INTEGER NOT NULL CHECK (sequence > 0),
    actor_id            TEXT NOT NULL REFERENCES participants(id),
    body                TEXT NOT NULL,
    reply_to_message_id TEXT REFERENCES messages(id),
    idempotency_key     TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    CHECK (typeof(id) = 'text'),
    CHECK (length(id) = 36),
    CHECK (substr(id, 9, 1) = '-'),
    CHECK (substr(id, 14, 1) = '-'),
    CHECK (substr(id, 19, 1) = '-'),
    CHECK (substr(id, 24, 1) = '-'),
    CHECK (substr(id, 15, 1) = '4'),
    CHECK (substr(id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (id = lower(id)),
    CHECK (instr(id, char(0)) = 0),
    CHECK (id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(id, '-', '')) = 32),
    UNIQUE (actor_id, idempotency_key),
    UNIQUE (room_id, sequence),
    CHECK (typeof(body) = 'text'),
    CHECK (length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    CHECK (instr(body, char(0)) = 0),
    CHECK (reply_to_message_id IS NULL OR typeof(reply_to_message_id) = 'text'),
    CHECK (reply_to_message_id IS NULL OR reply_to_message_id <> id),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*')
);

-- 写入兜底：Room active、actor Participant active、actor Membership
-- 周期 active（active_marker=1，004 周期模型）三者同时成立才允许插入；
-- 不写死 actor kind。
CREATE TRIGGER messages_insert_active_guard
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM rooms r
    JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = NEW.actor_id
    JOIN participants p ON p.id = NEW.actor_id
    WHERE r.id = NEW.room_id
      AND r.status = 'active'
      AND m.active_marker = 1
      AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'message requires an active room, active actor and active actor membership');
END;

-- 回复/引用目标写入兜底（契约 CORE_V1_REPLIES_AND_QUOTES.md 3.2/4）：
-- NEW.reply_to_message_id 非空时必须在插入时点证明 target 已是现存历史
-- Message（messages 行已提交）、target <> NEW.id（自指拒绝，含 CHECK 同
-- 行防自指的独立兜底）、与 source 同一 Room、且 target.sequence 落在
-- NEW.actor_id 的任一真实 Membership 周期窗口
-- （join_after_sequence < target.sequence 且 left_before_sequence 为空或
-- target.sequence <= left_before_sequence）——目标未知、跨 Room、对 actor
-- 历史不可见、自指一律 fail closed。应用层在同一事务内先做同一校验并
-- 返回稳定 400 invalid_reply_target，本守卫是对直接 SQL 的独立兜底；
-- 无 target（NULL）不改变普通消息路径。
CREATE TRIGGER messages_reply_target_guard
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NEW.reply_to_message_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1
      FROM messages t
      WHERE t.id = NEW.reply_to_message_id
        AND t.room_id = NEW.room_id
        AND t.id <> NEW.id
        AND EXISTS (
            SELECT 1 FROM room_memberships mm
            WHERE mm.room_id = NEW.room_id AND mm.participant_id = NEW.actor_id
              AND mm.join_after_sequence < t.sequence
              AND (mm.left_before_sequence IS NULL OR t.sequence <= mm.left_before_sequence)
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'reply target must be a same-room historical message visible to the actor');
END;

-- Message 公共字段与幂等协调列一经提交不可改写（等值自赋值不触发）。
CREATE TRIGGER messages_immutable
BEFORE UPDATE ON messages
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.sequence IS NOT OLD.sequence
  OR NEW.actor_id IS NOT OLD.actor_id
  OR NEW.body IS NOT OLD.body
  OR NEW.reply_to_message_id IS NOT OLD.reply_to_message_id
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'messages are immutable');
END;

-- Message 不允许硬删除。
CREATE TRIGGER messages_no_delete
BEFORE DELETE ON messages
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'messages cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (id)、UNIQUE (actor_id, idempotency_key)
-- 或 UNIQUE (room_id, sequence) 任一冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫替换既有 Message；本守卫在 INSERT 阶段直接
-- 拒绝任一身份已占用的写入。
CREATE TRIGGER messages_insert_identity_guard
BEFORE INSERT ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM messages
    WHERE id = NEW.id
       OR (actor_id = NEW.actor_id AND idempotency_key = NEW.idempotency_key)
       OR (room_id = NEW.room_id AND sequence = NEW.sequence)
)
BEGIN
    SELECT RAISE(ABORT, 'message identity already exists');
END;

-- mentions：显式结构化唤醒事实（契约 CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md
-- 4.4）。Mention 记录与 source Message 在同一事务提交：Message 先插入、
-- Mention 后插入；Mention 的任何守卫失败都会使整个 Message 事务回滚，
-- 不得留下 Message、Mention、Event 或 sequence。目标必须是 group 消息
-- 所在 Room 中 active 的 Agent 成员；非 group Room 的 Message 不接受
-- 任何 Mention（应用层拒绝非空 mentions，此处是持久层兜底）。
CREATE TABLE mentions (
    message_id      TEXT NOT NULL REFERENCES messages(id),
    target_agent_id TEXT NOT NULL REFERENCES participants(id),
    PRIMARY KEY (message_id, target_agent_id)
);

CREATE INDEX idx_mentions_target ON mentions(target_agent_id);

-- Mention 写入守卫：source Message 必须存在且其 Room 为 group；目标必须
-- 是 active Agent 且在该 Room 有 active Membership 周期（active_marker=1）。
-- 本守卫在 Mention INSERT 时点裁决，与 Message 同一事务，因此「提交事务
-- 内仍为 active」。
CREATE TRIGGER mentions_insert_guard
BEFORE INSERT ON mentions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM messages m
    JOIN rooms r ON r.id = m.room_id
    JOIN room_memberships mm
      ON mm.room_id = m.room_id AND mm.participant_id = NEW.target_agent_id AND mm.active_marker = 1
    JOIN participants p ON p.id = NEW.target_agent_id
    WHERE m.id = NEW.message_id
      AND r.kind = 'group'
      AND p.kind = 'agent' AND p.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'mention requires a group message and an active agent member target');
END;

-- Mention 事实一经提交不可改写。
CREATE TRIGGER mentions_immutable
BEFORE UPDATE ON mentions
FOR EACH ROW
WHEN NEW.message_id IS NOT OLD.message_id
  OR NEW.target_agent_id IS NOT OLD.target_agent_id
BEGIN
    SELECT RAISE(ABORT, 'mentions are immutable');
END;

-- Mention 不允许硬删除。
CREATE TRIGGER mentions_no_delete
BEFORE DELETE ON mentions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'mentions cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (message_id, target_agent_id) 冲突时先隐式
-- 删除冲突行，可绕过 no-delete / immutable 守卫替换 Mention 记录；本
-- 守卫在 INSERT 阶段直接拒绝 (message_id, target_agent_id) 已存在的写入。
CREATE TRIGGER mentions_insert_identity_guard
BEFORE INSERT ON mentions
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM mentions
    WHERE message_id = NEW.message_id AND target_agent_id = NEW.target_agent_id
)
BEGIN
    SELECT RAISE(ABORT, 'mention identity already exists');
END;

-- 上限兜底：单条 Message 至多 64 个 Mention 目标（契约
-- CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 4.4；与 ports/message.rs 的
-- MAX_MENTIONS_PER_MESSAGE 一致）。同一 message 已有 64 条时拒绝第 65 条，
-- 直接 SQL 也不能越界；本触发器与 source Message 同事务，第 65 条失败会
-- 使整个 Message 事务回滚，不得留下 Message、Mention、Event 或 sequence。
-- SQLite CHECK 不能引用其他行，因此用 BEFORE INSERT 触发器表达行数上限。
CREATE TRIGGER mentions_insert_max_guard
BEFORE INSERT ON mentions
FOR EACH ROW
WHEN (SELECT COUNT(*) FROM mentions WHERE message_id = NEW.message_id) >= 64
BEGIN
    SELECT RAISE(ABORT, 'a message can have at most 64 mentions');
END;
