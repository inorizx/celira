-- Celira Core V1：基础附件（Attachment）切片。
--
-- 契约：CORE_V1_ATTACHMENTS.md（唯一权威契约）
--
-- 事实模型：
-- - attachments：一条附件公共事实行。id（严格 UUID v4 主键）、
--   uploader_participant_id（上传者）、room_id（上传作用域）、file_name
--   （UTF-8 1-255 bytes、禁 NUL）、content_type（非空白 ASCII 可见
--   0x21-0x7E、1-255 bytes）、size_bytes（0..=16 MiB）、state
--   （pending/bound）、storage_key（内部随机 UUID v4，字节文件名唯一
--   来源）、sha256_hex（内部完整性事实，64 小写 hex）、uploaded_at（Core
--   分配）。公共协议绝不返回 storage_key / sha256_hex / 路径。
-- - pending 是私有暂存租约：上传后 24 小时内可被上传者在本 Room 绑定
--   一次；过期（uploaded_at + 24h <= now）后不可绑定、不可下载，只由
--   内部受控清理 `purge_expired_pending`（BEGIN IMMEDIATE 单写事务，
--   只删「过期 + state=pending + 无绑定行」的行）连同其字节一起回收。
--   bound 附件、绑定行与 Message/Event 公共事实绝不删除。
-- - message_attachments：Message 附件绑定关系（position 1..=8 有序；
--   PRIMARY KEY (message_id, position)、UNIQUE (message_id, attachment_id)、
--   UNIQUE (attachment_id)——一个附件只能绑定一次）。message_id 外键
--   DEFERRABLE INITIALLY DEFERRED：绑定行在 Message 行之前插入（同一事务），
--   由 messages 插入守卫在 Message 行插入时统一校验；孤儿绑定（无 Message）
--   在 COMMIT 被 deferred FK 拒绝。
-- - 绑定唯一允许的事实链（BEGIN IMMEDIATE）：插入 message_attachments →
--   插入 messages（守卫：body 与 attachments 至少一个 + 每个绑定为上传者
--   本人、同 Room、pending、未过期）→ messages AFTER INSERT 触发器把
--   附件 state 置为 bound（attachments_state_guard 只允许 pending→bound
--   且存在一致绑定）→ COMMIT。
-- - 守卫总览：附件与绑定行全字段不可 UPDATE、绑定行不可 DELETE；附件
--   行 DELETE 只允许「过期 + pending + 无绑定行」一种条件（内部受控清理
--   专用，见 attachments_no_delete），REPLACE 以任一身份冲突在插入阶段
--   直接拒绝；state 只能由同一事务的一致绑定驱动 pending→bound 一次；
--   messages 表整体重建（body 列改为可空），messages 的既有触发器全部
--   重建，body CHECK 改为「NULL 或 1-16384 bytes 无 NUL」。
-- - Human Event message.created 快照在发生时固化 attachments 数组
--   （{id,file_name,content_type,size_bytes}，position 升序，无附件为
--   json_array()）；撤回事务把该快照受守卫单向变形为 body=null、
--   recalled_at=首次时点、attachments=[]（human_event_facts_immutable 与
--   message_recalls_after_insert_converges 在本迁移重建；变形守卫不再要求
--   body 原为 text，改为以 recalled_at 未变形区分首次/二次变形，附件-only
--   消息的撤回与普通消息同一路径）。
-- - 旧开发数据库按破坏式收敛策略丢弃重建（不提供升级桥）；migration 012
--   只对全新库生效：messages 表在应用本迁移时必然为空，DROP TABLE +
--   CREATE TABLE 重建不会触发 FK 违约（readiness 继续要求全部 migration
--   已应用且 checksum 一致）。
-- - 本迁移不引入供应商、预览/缩略图/转码、SQLite BLOB、断点续传、异步
--   任务或 Runtime 对象。

-- ── 0. 破坏式收敛守卫：拒绝「已含 Message 事实的旧开发数据库」在途升级 ──
-- 本迁移重建 messages（body 列可空），无法对旧开发数据库原地升级（DROP
-- TABLE messages 会隐式删除既有 Message 行；若 FK 子表恰无引用行，会静默
-- 丢数据而不报错）。旧开发数据一律丢弃（CORE_V1_ATTACHMENTS.md §12 /
-- CORE_V1_MESSAGES.md §10）：任何在应用本迁移时已存在 messages 行的库
-- 都必须整体重建——这里显式拒绝在途升级（启动失败、readiness 不可达），
-- 绝不静默丢数据或半迁移。全新库（应用本迁移时 messages 必为空）不受
-- 影响。
CREATE TABLE _migration_012_messages_guard (
    guard INTEGER NOT NULL CHECK (guard = 0)
);
INSERT INTO _migration_012_messages_guard (guard)
SELECT 1 FROM messages LIMIT 1;
DROP TABLE _migration_012_messages_guard;

-- ── 1. 重建 messages：body 列可空（body 与 attachments 至少一个）──────────
-- 本迁移在全新库上执行，messages 为空表；DROP TABLE 同时移除其全部既有
-- 触发器（migration 005/007/009/011 定义的 messages 触发器随后全部重建）。
DROP TABLE messages;

CREATE TABLE messages (
    id                  TEXT PRIMARY KEY,
    room_id             TEXT NOT NULL REFERENCES rooms(id),
    sequence            INTEGER NOT NULL CHECK (sequence > 0),
    actor_id            TEXT NOT NULL REFERENCES participants(id),
    body                TEXT,
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
    CHECK (body IS NULL OR
           (typeof(body) = 'text'
            AND length(CAST(body AS BLOB)) BETWEEN 1 AND 16384
            AND instr(body, char(0)) = 0)),
    CHECK (reply_to_message_id IS NULL OR typeof(reply_to_message_id) = 'text'),
    CHECK (reply_to_message_id IS NULL OR reply_to_message_id <> id),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*')
);

-- 写入兜底：Room active、actor Participant active、actor Membership
-- 周期 active（active_marker=1，004 周期模型）三者同时成立才允许插入。
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

-- 回复/引用目标写入兜底（契约 CORE_V1_REPLIES_AND_QUOTES.md 3.2/4 +
-- CORE_V1_MESSAGE_RECALL.md 8）：target 必须同 Room、已提交、对 actor 任一
-- 历史 Membership 周期窗口可见且未撤回（存在 message_recalls 即拒绝）。
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
        AND NOT EXISTS (
            SELECT 1 FROM message_recalls r WHERE r.message_id = t.id
        )
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

-- 附件一致性守卫（契约 CORE_V1_ATTACHMENTS.md 9.1）：Message 必须满足
-- 「body 非空或至少一个绑定」，且每个绑定必须指向存在、state=pending、
-- 上传者==Message actor、同 Room、未超过 24 小时绑定窗口的附件。绑定行在
-- 本 INSERT 之前已由应用层插入同一事务（deferred FK），因此本守卫能完整
-- 裁决；直接 SQL 无法用 body=NULL 无绑定的 Message 或跨 actor/跨 Room/
-- 已 bound/已过期的绑定绕过。
CREATE TRIGGER messages_attachments_guard
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NOT (
    (NEW.body IS NOT NULL
     OR EXISTS (SELECT 1 FROM message_attachments ma WHERE ma.message_id = NEW.id))
    AND NOT EXISTS (
        SELECT 1
        FROM message_attachments ma
        JOIN attachments a ON a.id = ma.attachment_id
        WHERE ma.message_id = NEW.id
          AND NOT (
              a.uploader_participant_id = NEW.actor_id
              AND a.room_id = NEW.room_id
              AND a.state = 'pending'
              AND a.uploaded_at + 86400000 >= NEW.created_at
          )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'message requires a body or pending attachments bound by the actor in the same room');
END;

-- 绑定落定：Message 行成立后，同一事务内把其全部附件置为 bound。
-- attachments_state_guard 独立校验该转换的合法性（见下）。
CREATE TRIGGER messages_attachments_bound_after_insert
AFTER INSERT ON messages
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM message_attachments ma WHERE ma.message_id = NEW.id)
BEGIN
    UPDATE attachments SET state = 'bound'
    WHERE id IN (SELECT attachment_id FROM message_attachments WHERE message_id = NEW.id);
END;

-- Message 公共字段与幂等协调列一经提交不可改写（等值自赋值不触发；
-- body 可为 NULL，NULL 等值比较不受影响）。
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

-- 身份防改写：REPLACE 以主键/唯一约束任一冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫；本守卫在 INSERT 阶段直接拒绝任一身份已占用的
-- 写入。
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

-- 新普通 Message 的 Event 生成（direct 路径，migration 007 原样重建）。
CREATE TRIGGER messages_after_insert_generates_agent_event
AFTER INSERT ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id AND p.human_id = NEW.actor_id
)
BEGIN
    INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.created',
        p.agent_id,
        (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
         WHERE target_agent_id = p.agent_id),
        NEW.id,
        NEW.created_at
    FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id AND p.human_id = NEW.actor_id
      AND EXISTS (SELECT 1 FROM participants a
                  WHERE a.id = p.agent_id AND a.kind = 'agent' AND a.status = 'active')
      AND EXISTS (SELECT 1 FROM room_memberships mm
                  WHERE mm.room_id = p.room_id
                    AND mm.participant_id = p.agent_id
                    AND mm.active_marker = 1);
END;

-- ── 2. attachments / message_attachments 事实表 ──────────────────────────
CREATE TABLE attachments (
    id                      TEXT PRIMARY KEY,
    uploader_participant_id TEXT NOT NULL REFERENCES participants(id),
    room_id                 TEXT NOT NULL REFERENCES rooms(id),
    file_name               TEXT NOT NULL,
    content_type            TEXT NOT NULL,
    size_bytes              INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 16777216),
    state                   TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'bound')),
    storage_key             TEXT NOT NULL UNIQUE,
    sha256_hex              TEXT NOT NULL,
    uploaded_at             INTEGER NOT NULL,
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
    CHECK (typeof(storage_key) = 'text'),
    CHECK (length(storage_key) = 36),
    CHECK (substr(storage_key, 9, 1) = '-'),
    CHECK (substr(storage_key, 14, 1) = '-'),
    CHECK (substr(storage_key, 19, 1) = '-'),
    CHECK (substr(storage_key, 24, 1) = '-'),
    CHECK (substr(storage_key, 15, 1) = '4'),
    CHECK (substr(storage_key, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (storage_key = lower(storage_key)),
    CHECK (instr(storage_key, char(0)) = 0),
    CHECK (storage_key NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(storage_key, '-', '')) = 32),
    CHECK (typeof(uploader_participant_id) = 'text'),
    CHECK (instr(uploader_participant_id, char(0)) = 0),
    CHECK (typeof(room_id) = 'text'),
    CHECK (instr(room_id, char(0)) = 0),
    CHECK (typeof(file_name) = 'text'),
    CHECK (length(CAST(file_name AS BLOB)) BETWEEN 1 AND 255),
    CHECK (instr(file_name, char(0)) = 0),
    CHECK (typeof(content_type) = 'text'),
    CHECK (length(content_type) BETWEEN 1 AND 255),
    CHECK (content_type NOT GLOB '*[^!-~]*'),
    CHECK (typeof(size_bytes) = 'integer'),
    CHECK (typeof(state) = 'text'),
    CHECK (typeof(sha256_hex) = 'text'),
    CHECK (length(sha256_hex) = 64),
    CHECK (sha256_hex = lower(sha256_hex)),
    CHECK (sha256_hex NOT GLOB '*[^0-9a-f]*'),
    CHECK (typeof(uploaded_at) = 'integer')
);

CREATE INDEX idx_attachments_room ON attachments(room_id);
CREATE INDEX idx_attachments_uploader ON attachments(uploader_participant_id);

CREATE TABLE message_attachments (
    message_id    TEXT NOT NULL REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED,
    attachment_id TEXT NOT NULL REFERENCES attachments(id),
    position      INTEGER NOT NULL CHECK (position BETWEEN 1 AND 8),
    PRIMARY KEY (message_id, position),
    UNIQUE (message_id, attachment_id),
    UNIQUE (attachment_id),
    CHECK (typeof(message_id) = 'text'),
    CHECK (typeof(attachment_id) = 'text'),
    CHECK (typeof(position) = 'integer')
);

CREATE INDEX idx_message_attachments_attachment ON message_attachments(attachment_id);

-- ── 3. attachments / message_attachments 触发器 ──────────────────────────
-- 附件事实一经提交不可改写（state 例外：见 attachments_state_guard）。
CREATE TRIGGER attachments_immutable
BEFORE UPDATE ON attachments
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
  OR NEW.uploader_participant_id IS NOT OLD.uploader_participant_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.file_name IS NOT OLD.file_name
  OR NEW.content_type IS NOT OLD.content_type
  OR NEW.size_bytes IS NOT OLD.size_bytes
  OR NEW.storage_key IS NOT OLD.storage_key
  OR NEW.sha256_hex IS NOT OLD.sha256_hex
  OR NEW.uploaded_at IS NOT OLD.uploaded_at
BEGIN
    SELECT RAISE(ABORT, 'attachments are immutable');
END;

-- state 唯一合法转换：pending → bound，且必须存在一条一致绑定
-- （绑定行 + 同 Room、uploader==Message actor 的 Message）。直接 SQL 无法
-- 反向解绑、越权置 bound 或伪造转换。
CREATE TRIGGER attachments_state_guard
BEFORE UPDATE OF state ON attachments
FOR EACH ROW
WHEN NOT (
    OLD.state = 'pending' AND NEW.state = 'bound'
    AND EXISTS (
        SELECT 1 FROM message_attachments ma JOIN messages m ON m.id = ma.message_id
        WHERE ma.attachment_id = NEW.id
          AND m.actor_id = NEW.uploader_participant_id
          AND m.room_id = NEW.room_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'attachment state can only move once from pending to bound with a consistent binding');
END;

-- 附件删除守卫（pending 私有暂存租约的受控回收）：只允许删除「已过期
-- （uploaded_at + 24h <= 当前墙钟）、state 仍为 pending、且无任何绑定行」
-- 的附件行——这是 `purge_expired_pending` 内部受控清理的唯一合法条件。
-- 其余（未过期 pending、bound、存在绑定行的附件）一律拒绝；直接 SQL 用
-- 该条件删除过期无绑定 pending 行是允许的（内部清理语义），但绝不允许
-- 借此删除任何公共事实。清理事务与绑定事务并发时由 BEGIN IMMEDIATE
-- 写锁线性化，绑定先落则行变为 bound/带绑定、清理不可见。
CREATE TRIGGER attachments_no_delete
BEFORE DELETE ON attachments
FOR EACH ROW
WHEN NOT (
    OLD.state = 'pending'
    AND OLD.uploaded_at + 86400000 <= CAST(strftime('%s', 'now') AS INTEGER) * 1000
    AND NOT EXISTS (
        SELECT 1 FROM message_attachments ma WHERE ma.attachment_id = OLD.id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'attachments can only be deleted when expired, pending and unbound');
END;

-- 身份防改写：REPLACE 以主键 (id) 或 UNIQUE (storage_key) 冲突时先隐式
-- 删除冲突行；本守卫在 INSERT 阶段直接拒绝已占用的写入。
CREATE TRIGGER attachments_insert_identity_guard
BEFORE INSERT ON attachments
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM attachments
    WHERE id = NEW.id OR storage_key = NEW.storage_key
)
BEGIN
    SELECT RAISE(ABORT, 'attachment identity already exists');
END;

-- 绑定写入守卫：附件必须存在且 pending（未绑定过）、同一附件不得二次
-- 绑定；当目标 Message 已存在（直接 SQL 攻击路径）时，还必须在插入时点
-- 证明 uploader==Message actor 且同 Room（正规绑定先于 Message 行插入，
-- 该分支不触发，一致性由 messages_attachments_guard 在 Message 行插入时
-- 统一裁决）。
CREATE TRIGGER message_attachments_insert_guard
BEFORE INSERT ON message_attachments
FOR EACH ROW
WHEN NOT (
    EXISTS (SELECT 1 FROM attachments a WHERE a.id = NEW.attachment_id AND a.state = 'pending')
    AND NOT EXISTS (SELECT 1 FROM message_attachments ma WHERE ma.attachment_id = NEW.attachment_id)
    AND (
        NOT EXISTS (SELECT 1 FROM messages m WHERE m.id = NEW.message_id)
        OR EXISTS (SELECT 1 FROM messages m JOIN attachments a ON a.id = NEW.attachment_id
                   WHERE m.id = NEW.message_id
                     AND a.uploader_participant_id = m.actor_id
                     AND a.room_id = m.room_id)
    )
)
BEGIN
    SELECT RAISE(ABORT, 'attachment binding requires a pending attachment of the message actor in the same room');
END;

-- 绑定行一经提交不可改写。
CREATE TRIGGER message_attachments_immutable
BEFORE UPDATE ON message_attachments
FOR EACH ROW
WHEN NEW.message_id IS NOT OLD.message_id
  OR NEW.attachment_id IS NOT OLD.attachment_id
  OR NEW.position IS NOT OLD.position
BEGIN
    SELECT RAISE(ABORT, 'message attachments are immutable');
END;

-- 绑定行不允许硬删除（绑定后不可解绑）。
CREATE TRIGGER message_attachments_no_delete
BEFORE DELETE ON message_attachments
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'message attachments cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (message_id, position)、UNIQUE
-- (message_id, attachment_id) 或 UNIQUE (attachment_id) 任一冲突时先隐式
-- 删除冲突行；本守卫在 INSERT 阶段直接拒绝已占用的写入。
CREATE TRIGGER message_attachments_insert_identity_guard
BEFORE INSERT ON message_attachments
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM message_attachments
    WHERE (message_id = NEW.message_id AND position = NEW.position)
       OR (message_id = NEW.message_id AND attachment_id = NEW.attachment_id)
       OR (attachment_id = NEW.attachment_id)
)
BEGIN
    SELECT RAISE(ABORT, 'message attachment identity already exists');
END;

-- ── 4. 重建 human_event_facts_message_created：快照固化 attachments ──────
-- 绑定行先于 Message 行插入同一事务，触发器在 Message 行成立时能直接读取
-- message_attachments 构建完整快照（position 升序；无绑定为 json_array()）。
-- json_group_array 的输入子查询带 ORDER BY position，SQLite 按该顺序聚合
-- （bundled 3.46.0 实测）；外层 json() 避免数组被二次编码为字符串。
-- （本触发器附着于 messages 表，已在第 1 步 DROP TABLE messages 时随表
-- 移除，此处直接重建，不需要也不允许再 DROP TRIGGER。）
CREATE TRIGGER human_event_facts_message_created
AFTER INSERT ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_memberships mm
    JOIN participants p ON p.id = mm.participant_id
    WHERE mm.room_id = NEW.room_id AND mm.active_marker = 1 AND p.kind = 'human'
)
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.created',
        NEW.id,
        NEW.room_id,
        NEW.created_at,
        json_object(
            'id', NEW.id,
            'room_id', NEW.room_id,
            'actor', json_object(
                'id', p.id,
                'kind', p.kind,
                'display_name', p.display_name,
                'status', p.status
            ),
            'body', NEW.body,
            'reply_to_message_id', NEW.reply_to_message_id,
            'created_at', NEW.created_at,
            'recalled_at', NULL,
            'attachments', json(COALESCE(
                (SELECT json_group_array(json_object(
                        'id', x.id,
                        'file_name', x.file_name,
                        'content_type', x.content_type,
                        'size_bytes', x.size_bytes))
                 FROM (SELECT a.id, a.file_name, a.content_type, a.size_bytes
                       FROM message_attachments ma JOIN attachments a ON a.id = ma.attachment_id
                       WHERE ma.message_id = NEW.id
                       ORDER BY ma.position) AS x),
                json_array()
            ))
        )
    FROM participants p
    WHERE p.id = NEW.actor_id;
END;

-- ── 5. 重建 human_event_facts_immutable：撤回变形包含 attachments=[] ──────
-- 唯一允许的快照修改：message.created 快照在有精确 message_recalls 见证时
-- 的一次 active shape → exact tombstone 变形（body→null、recalled_at→首次
-- 时点、attachments→[]）。防线以完整 JSON 文本精确相等收口：允许的新快照
-- 必须与 OLD 经唯一变换
--   json_set(OLD.snapshot, '$.body', NULL, '$.recalled_at', <witness>,
--            '$.attachments', json_array())
-- 的完整结果逐字相等。附件-only 消息（创建时 body 已为 null）的首次撤回
-- 同样合法：以「recalled_at 未变形」区分首次与二次变形（不再要求 body 原
-- 为 text）；其余防线不变（无见证、反向恢复、增删/重排键、嵌套篡改、改
-- 其他键、二次变形一律拒绝）。
DROP TRIGGER human_event_facts_immutable;

CREATE TRIGGER human_event_facts_immutable
BEFORE UPDATE ON human_event_facts
FOR EACH ROW
WHEN NOT (
    NEW.fact_id IS OLD.fact_id
    AND NEW.kind IS OLD.kind
    AND NEW.source_id IS OLD.source_id
    AND NEW.room_id IS OLD.room_id
    AND NEW.created_at IS OLD.created_at
    AND (
        NEW.snapshot IS OLD.snapshot
        OR (
            OLD.kind = 'message.created'
            AND COALESCE(json_type(OLD.snapshot, '$.recalled_at'), 'null') = 'null'
            AND EXISTS (
                SELECT 1 FROM message_recalls r
                WHERE r.message_id = NEW.source_id
                  AND NEW.snapshot IS json_set(
                      OLD.snapshot, '$.body', NULL, '$.recalled_at', r.recalled_at,
                      '$.attachments', json_array()
                  )
            )
        )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'human event facts are immutable');
END;

-- ── 6. 重建 message_recalls_after_insert_converges：created 快照变形清空 ──
-- 附件（与 011 原版唯一差异：变形 UPDATE 追加 '$.attachments', json_array()）。
DROP TRIGGER message_recalls_after_insert_converges;

CREATE TRIGGER message_recalls_after_insert_converges
AFTER INSERT ON message_recalls
FOR EACH ROW
BEGIN
    INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.recalled',
        NEW.message_id,
        m.room_id,
        NEW.recalled_at,
        json_object(
            'message_id', NEW.message_id,
            'room_id', m.room_id,
            'recalled_at', NEW.recalled_at
        )
    FROM messages m
    WHERE m.id = NEW.message_id
      AND EXISTS (SELECT 1 FROM human_event_facts c
                  WHERE c.kind = 'message.created' AND c.source_id = NEW.message_id);

    UPDATE human_event_facts
    SET snapshot = json_set(
            snapshot,
            '$.body', NULL,
            '$.recalled_at', NEW.recalled_at,
            '$.attachments', json_array()
        )
    WHERE kind = 'message.created' AND source_id = NEW.message_id;

    INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at)
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-' ||
        '4' || substr(lower(hex(randomblob(2))), 2) || '-' ||
        substr('89ab', abs(random() % 4) + 1, 1) || substr(lower(hex(randomblob(2))), 2) || '-' ||
        lower(hex(randomblob(6))),
        'message.recalled',
        e.target_agent_id,
        (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events
         WHERE target_agent_id = e.target_agent_id),
        e.message_id,
        NEW.recalled_at
    FROM agent_events e
    WHERE e.kind = 'message.created' AND e.message_id = NEW.message_id;
END;
