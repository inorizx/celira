-- Celira Core V1：Room 容器、Membership 周期、Contact 与 Human Direct / Group 切片。
--
-- 契约：CORE_V1_DIRECT_ROOMS.md + CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md
-- 本迁移把 rooms 破坏式收敛为统一公共容器，kind 支持三种：
--   direct        既有 Human–Agent Direct Room（语义不变）
--   human_direct  两个 accepted Human 的私聊
--   group         多 Human/Agent Participant 的群聊容器
-- 旧开发数据一律丢弃；不提供任何旧 schema 升级桥、checksum 兼容或双读。
--
-- Membership 周期模型（本迁移的核心破坏式收敛）：
-- - room_memberships 每行是一条不可变 Membership 周期：独立
--   membership_id（UUID v4）主键；room_id、participant_id、role、joined_at、
--   join_after_sequence、left_at、left_before_sequence、closed_by、
--   active_marker。周期窗口由 Room 内不可变 sequence 裁决：
--   join_after_sequence < message.sequence 且 (left_before_sequence IS NULL
--   或 message.sequence <= left_before_sequence)；时间只作展示与审计，不参与
--   授权；
-- - active_marker 与 left 三元组（left_at、left_before_sequence、closed_by）
--   用 CHECK 保持一致：active 周期三个 left 字段全 NULL、active_marker=1；
--   关闭后三个字段全非 NULL、active_marker=0，整行不可变；关闭只能由
--   UPDATE 路径产生（INSERT 只能创建 active 周期）；
-- - 同 Room+Participant 至多一个 active 周期（部分唯一索引 WHERE
--   active_marker=1；closed 历史周期不占位，可累积多条）；每 Room 至多
--   一个 active owner（部分唯一索引 WHERE role='owner' AND active_marker=1）；
--   关闭旧周期再打开新周期（role 列每行不可变）；
-- - 关闭原因 closed_by ∈ ('leave','removed','transfer','agent_revoked')：
--   leave 只有 group 成员主动退群；removed 是 group owner 移除；
--   transfer 是群主原子转让（旧 owner 与目标 member 都关闭、新 owner 与
--   旧 owner member 都打开）；agent_revoked 是 Agent 撤销传播（触发器在
--   participants 同一事务内用 NEW.revoked_at 关闭其全部 active 周期，每个
--   Room 固化 left_before_sequence = 该 Room 当前最大 message sequence，
--   不跨 adapter 双写）。group 的 owner 周期只能经合法 transfer 关闭：
--   closed_by='leave'/'removed'/'agent_revoked' 直接被关闭守卫拒绝；
--   'transfer' 必须在同一事务内由完整转让支撑——transfer 关闭守卫要求
--   当前事务已存在匹配的 transition 事实（见下），把转让钉死为单事务
--   原子动作：单独以 closed_by='transfer' 关闭旧 owner 或目标 member
--   都会在写入时立即失败，无法拆成两笔事务；随后打开新 owner 与旧 owner
--   member、更新 coverage 见证再由 COMMIT 期 deferred 复合外键兜底；
--   新 owner 周期必须有合法来路——是该 group 的首个 owner
--   （该 Room 尚无任何 Membership 周期）或同一事务内存在精确匹配
--   membership_id/room/目标/joined_at/join_after 的 group_owner_transitions
--   事实（见下）。从不看模糊历史：某人历史上有任一 transfer 周期这一事实
--   本身不能让无 active 周期的他再次成为 owner；
-- - room_coverage 是每个 Room 唯一的 kind-specific 结构见证：direct 的
--   Human–Agent 配对、human_direct 的 Human–Human 配对、group 的 owner。
--   group 分支以复合外键 (room_id, group_owner_membership_id,
--   group_owner_role, group_owner_active_marker) 见证 room_memberships 的
--   UNIQUE (room_id, membership_id, role, active_marker)，CHECK 限定
--   group_owner_role='owner' 且 group_owner_active_marker=1——coverage 只能
--   见证 group 中同 room、role=owner、active_marker=1 的周期；transfer 时
--   先关闭旧 owner、再打开新 owner、再更新 coverage 见证；任何一步缺失
--   （如只关闭 owner 不转让）都会在 COMMIT 时因 deferred 外键失败；
--   direct/human_direct 配对只见证奠基 Membership 身份与固定角色（复合外
--   键指向 membership_id + role，不要求 Agent 永远 active）；rooms →
--   room_coverage → 配对/owner membership 都是 DEFERRABLE INITIALLY
--   DEFERRED 复合外键，在 COMMIT 时反向证明「已提交的 Room 必须自证结构
--   闭合」；CHECK 保证每个 kind 恰好填其所需见证字段；覆盖 kind 必须等于
--   Room kind；
-- - direct_room_pairs 记录 Human–Agent 配对：一房一条配对事实（主键
--   room_id），同一 Human–Agent 对可以拥有多个 Direct Room——同对唯一
--   不再成立（不再有 UNIQUE(human_id, agent_id)），方向必须是
--   Human → Agent 且双方 active，创建时该 Human 对该 Agent 存在 active
--   Agent Owner 关系；配对以 human_membership_id / agent_membership_id
--   见证奠基 Membership 身份（FK 到 (room_id, membership_id, role)，Agent
--   撤销关闭周期后配对仍成立）；配对身份、所属 Room 与 created_at 不可
--   改写；覆盖 kind 必须为 direct；
-- - human_direct_pairs 记录两个 accepted Human 的私聊配对：按规范顺序
--   （human_a_id < human_b_id）唯一（同一对 Human 最多一个 Room），
--   创建者恒为 owner（owner 跟随创建者位置，canonical 顺序只保证同一对
--   唯一；role_a/role_b 由 CHECK 限定两端 role 互异且 owner/member 各一），
--   双方必须 active；配对复合外键指向 contacts(a_id, b_id) 且必须为
--   accepted（同一事务内被邀请 Human 不需要再次确认）；配对身份不可改写；
-- - contacts 是 Human–Human 对称单事实：同一对 Human 只有一条关系
--   （规范 (a_id, b_id) 主键，a_id < b_id），requester/addressee 方向由
--   CHECK 限定为配对内两端且互不相同；A→B 与 B→A 并发收敛到同一条事实；
--   pending → accepted 是唯一状态迁移，只由 addressee 的接受动作发起，
--   accepted 是终态（accepted_at 必有且不可改写）；请求幂等键按
--   (requester_id, key) 唯一，只用于创建事实，不进入公开联系人视图；
-- - group 没有配对表：成员直接进入 room_memberships（不造邀请状态机），
--   group 恰好一个 active owner（第一个 owner 周期为创建者，转移只经
--   transfer 事务），owner 只能邀请 accepted Human 或自己拥有且 active 的
--   Agent（INSERT 守卫在同一事务内证明资格）；invite 必须携带
--   Idempotency-Key 并持久化到 group_member_invite_idempotency：每个成功
--   key 记录 human+key → room+participant+membership_id，先查 key 再查
--   权限；同 key 重试（即使该周期后来关闭）只重放历史结果，绝不误开新
--   周期；重新邀请必须使用新 key；不同 key 并发同目标最终只有一个 active
--   周期，后续请求各自记录到该 active 周期并返回 200；目标有历史 closed
--   周期时只要资格仍满足即可用新 key 重入（201），不再有 MemberRevoked
--   终态；「目标已有 active 周期返回 200」不包括 owner 自己：owner 邀请
--   自己显式识别并返回稳定 404 member_not_found，不写幂等记录、不进
--   DB；
-- - 群邀请/重入幂等记录：group_member_invite_idempotency 的 (room_id,
--   participant_id, membership_id) 以 DEFERRABLE 复合外键（parent key：
--   room_memberships UNIQUE (room_id, participant_id, membership_id)）
--   对应同一条真实周期行，不能只引用 membership_id 而让 participant/
--   room 指向另一事实；写入时守卫证明该周期当时是 group member（不是
--   owner）；周期之后关闭不影响历史 key 重放；
-- - 每次群主转让都是一条不可变事实 group_owner_transitions：transition_id
--   （UUID v4）主键；room_id、old_owner_participant_id、
--   target_participant_id、四条 Membership 周期 ID（old_owner_membership_id
--   = 被关闭的旧 owner 周期、target_member_membership_id = 被关闭的目标
--   member 周期、new_owner_membership_id = 打开的目标 owner 周期、
--   old_owner_new_member_membership_id = 打开的旧 owner member 周期）、
--   occurred_at（= 四条周期的时间）、boundary_sequence（= 四条周期的
--   sequence 边界，>= 0）。四条周期以 DEFERRABLE 复合外键精确见证同
--   Room 的 room_memberships 行：旧 owner（participant=旧 owner、
--   role=owner、left_at=occurred_at、left_before=boundary、
--   closed_by=transfer）、目标旧 member（participant=目标、role=member、
--   同一时间/边界/关闭原因）、新 owner（participant=目标、role=owner、
--   joined_at=occurred_at、join_after=boundary）、旧 owner 新 member
--   （participant=旧 owner、role=member、同一时间/边界）；新旧四条周期
--   各自在对应的 transition 角色列 UNIQUE，防止同一周期被重复消费（同一
--   周期可先作为某次 new_owner、之后作为下一次 old_owner——不同角色列，
--   允许）；transition 整行 immutable / no-delete / REPLACE guard；
--   transition 还有 INSERT guard：插入时必须证明旧 owner 与目标 member
--   周期当时都 active（group Room、正确 room/participant/role、两端
--   active Human、old owner 是该 Room 唯一 active owner）——因此转让事务
--   必须先插 transition 再关闭，transition 是本次四周期原子转让的见证，
--   任何单独 transfer 关闭（T1 拆走目标 member 关闭）都被关闭守卫当场
--   拒绝；
-- - group owner 转让（POST /v1/rooms/{room_id}/owner-transfer）使用持久
--   幂等表并先查 key：在一个 BEGIN IMMEDIATE 事务中读取一次 X=当前 Room
--   最大 message sequence 与时间 T；生成 transition_id 与两条新周期 ID；
--   插入 transition（此时旧 owner 与目标 member 仍 active，INSERT guard
--   通过；其新周期 FK deferred，先于新周期插入）；关闭旧 owner 与目标
--   member（closed_by='transfer'，left_before_sequence=X——transfer 关闭
--   守卫在事务内找到刚插入的匹配 transition）；打开目标 owner 与旧 owner
--   member（join_after_sequence=X）；更新 coverage 见证；写幂等记录。
--   任何一步失败整体回滚；转让目标只能是当前 active 的 Human member 且
--   不能是自己；Agent、revoked、非成员返回稳定业务错误；幂等记录收敛为
--   (human_id, key) → transition_id：复合外键证明 human_id 正是
--   transition.old_owner_participant_id，Room/目标/新 owner 事实只从
--   不可变 transition 读取、不在幂等表重复；FK 不绑定 active_marker，
--   新 Owner 再次转让后旧 key 仍可合法历史重放；
-- - 幂等记录：direct_room_idempotency（human+key→配对+Room+name，name
--   见证请求的规范化标题含 null）、
--   human_direct_room_idempotency（human+key→另一 Human+Room）、
--   group_create_idempotency（human+key→Room+name）、
--   group_member_invite_idempotency（human+key→room+participant+
--   membership_id）、group_owner_transfer_idempotency（human+key→room+
--   target+new_owner_membership_id）都以 (human_id, key) 唯一并复合关联
--   真实配对/覆盖/周期事实（幂等 witness 以 DEFERRABLE 复合外键指向同一
--   条真实 room_memberships 行 + 写入时角色/active 守卫），整行一经创建
--   不可改写；direct_room_idempotency 的 name 列写入时由触发器见证必须
--   等于 rooms.name（room 与标题都不可变，写入时证明即终局证明）；
-- - REPLACE 防线：SQLite 的 INSERT OR REPLACE / REPLACE 在身份冲突时先
--   隐式删除冲突行，默认 recursive_triggers=OFF 下不会触发 no-delete
--   触发器，可绕过 immutable / no-delete 守卫改写或重建既有事实；因此
--   本迁移每个不可变公共事实表（contacts、rooms、room_coverage、
--   direct_room_pairs、human_direct_pairs、room_memberships、
--   group_owner_transitions、direct_room_idempotency、
--   human_direct_room_idempotency、
--   group_create_idempotency、group_member_invite_idempotency、
--   group_owner_transfer_idempotency）都配 BEFORE INSERT identity guard，
--   在插入阶段直接拒绝任何身份已存在的写入（普通重复 INSERT 同样被拒）；
--   应用层幂等重放先查询后插入，不依赖数据库重复插入；
-- - 本迁移不引入 Message、Backend、Credential、Runtime 或任何延期对象。
--
-- 表创建顺序利用 SQLite 对「外键父表尚未存在」的宽容：先建配对表
-- （其 FK 惰性引用后建的 memberships），再建 memberships（其触发器引用
-- 已存在的配对表），运行时 DML 才解析外键。

CREATE TABLE contacts (
    a_id           TEXT NOT NULL REFERENCES participants(id),
    b_id           TEXT NOT NULL REFERENCES participants(id),
    requester_id   TEXT NOT NULL REFERENCES participants(id),
    addressee_id   TEXT NOT NULL REFERENCES participants(id),
    status         TEXT NOT NULL CHECK (status IN ('pending', 'accepted')),
    created_at     INTEGER NOT NULL,
    accepted_at    INTEGER,
    idempotency_key TEXT NOT NULL,
    PRIMARY KEY (a_id, b_id),
    CHECK (a_id < b_id),
    CHECK ((requester_id = a_id AND addressee_id = b_id)
        OR (requester_id = b_id AND addressee_id = a_id)),
    CHECK (requester_id <> addressee_id),
    CHECK ((status = 'accepted' AND accepted_at IS NOT NULL)
        OR (status = 'pending' AND accepted_at IS NULL)),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (idempotency_key = trim(idempotency_key)),
    CHECK (instr(idempotency_key, char(0)) = 0),
    CHECK (instr(idempotency_key, ',') = 0),
    CHECK (idempotency_key NOT GLOB '*[^!-~]*'),
    UNIQUE (requester_id, idempotency_key)
);

CREATE INDEX idx_contacts_participant ON contacts(requester_id);
CREATE INDEX idx_contacts_addressee ON contacts(addressee_id);

-- 新联系人关系：两端必须是不同且 active 的 Human。
CREATE TRIGGER contacts_insert_guard
BEFORE INSERT ON contacts
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM participants p1 WHERE p1.id = NEW.a_id AND p1.kind = 'human' AND p1.status = 'active'
) OR NOT EXISTS (
    SELECT 1 FROM participants p2 WHERE p2.id = NEW.b_id AND p2.kind = 'human' AND p2.status = 'active'
)
BEGIN
    SELECT RAISE(ABORT, 'contact requires two different active humans');
END;

-- 状态机：pending → accepted 是唯一迁移（必须同时写入 accepted_at）；
-- accepted 是终态，不得改回 pending；accepted_at 一经写入不可改写。
CREATE TRIGGER contacts_status_machine
BEFORE UPDATE OF status, accepted_at ON contacts
FOR EACH ROW
WHEN (OLD.status = 'pending' AND NEW.status = 'accepted' AND NEW.accepted_at IS NULL)
  OR (OLD.status = 'accepted' AND NEW.status = 'pending')
  OR (OLD.status = 'accepted' AND NEW.status <> 'accepted')
  OR (OLD.accepted_at IS NOT NULL AND NEW.accepted_at IS NOT OLD.accepted_at)
BEGIN
    SELECT RAISE(ABORT, 'contact status can only move once from pending to accepted');
END;

-- 联系人关系身份（配对、方向、幂等键、创建时间）不可改写。
CREATE TRIGGER contacts_identity_immutable
BEFORE UPDATE OF a_id, b_id, requester_id, addressee_id, idempotency_key, created_at
ON contacts
FOR EACH ROW
WHEN NEW.a_id IS NOT OLD.a_id
  OR NEW.b_id IS NOT OLD.b_id
  OR NEW.requester_id IS NOT OLD.requester_id
  OR NEW.addressee_id IS NOT OLD.addressee_id
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'contact identity is immutable');
END;

-- 联系人关系不允许硬删除。
CREATE TRIGGER contacts_no_delete
BEFORE DELETE ON contacts
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'contacts cannot be deleted');
END;

-- 身份防改写：INSERT OR REPLACE / REPLACE 在主键 (a_id, b_id) 或
-- UNIQUE (requester_id, idempotency_key) 冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫把 accepted 改回 pending 或改写方向与幂等键；
-- 本守卫在 INSERT 阶段直接拒绝任何身份已占用的写入。
CREATE TRIGGER contacts_insert_identity_guard
BEFORE INSERT ON contacts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM contacts
    WHERE (a_id = NEW.a_id AND b_id = NEW.b_id)
       OR (requester_id = NEW.requester_id AND idempotency_key = NEW.idempotency_key)
)
BEGIN
    SELECT RAISE(ABORT, 'contact identity already exists');
END;

CREATE TABLE rooms (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('direct', 'human_direct', 'group')),
    status     TEXT NOT NULL CHECK (status IN ('active')),
    created_at INTEGER NOT NULL,
    name       TEXT,
    CHECK (name IS NULL
        OR (length(name) <= 80 AND instr(name, char(0)) = 0
            AND trim(name, char(9) || char(10) || char(11) || char(12) || char(13) || char(32) ||
                char(133) || char(160) || char(5760) ||
                char(8192) || char(8193) || char(8194) || char(8195) || char(8196) || char(8197) ||
                char(8198) || char(8199) || char(8200) || char(8201) || char(8202) || char(8232) ||
                char(8233) || char(8239) || char(8287) || char(12288)) <> '')),
    FOREIGN KEY (id) REFERENCES room_coverage(room_id)
        DEFERRABLE INITIALLY DEFERRED
);

-- Room 身份、容器状态、群名与创建时间不可改写；不允许硬删除。
CREATE TRIGGER rooms_identity_immutable
BEFORE UPDATE OF id, kind, status, created_at, name ON rooms
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id OR NEW.kind IS NOT OLD.kind OR NEW.status IS NOT OLD.status
  OR NEW.created_at IS NOT OLD.created_at OR NEW.name IS NOT OLD.name
BEGIN
    SELECT RAISE(ABORT, 'room identity, status and created_at are immutable');
END;

CREATE TRIGGER rooms_no_delete
BEFORE DELETE ON rooms
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'rooms cannot be deleted');
END;

-- 身份防改写：REPLACE 以 rooms 主键 (id) 冲突时先隐式删除再重建，可绕过
-- no-delete / immutable 守卫改写 kind / created_at / name；本守卫在
-- INSERT 阶段直接拒绝 id 已存在的写入。
CREATE TRIGGER rooms_insert_identity_guard
BEFORE INSERT ON rooms
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM rooms WHERE id = NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'room identity already exists');
END;

-- kind 与标题的形状守则：direct 与 group 允许可选非空标题
-- （非 NULL 时由 rooms 表级 CHECK 保证形状），human_direct 恒强制 NULL
-- （Human–Human 私聊没有标题事实源）。
CREATE TRIGGER rooms_insert_kind_name_guard
BEFORE INSERT ON rooms
FOR EACH ROW
WHEN NEW.kind = 'human_direct' AND NEW.name IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'human_direct rooms must have a null name');
END;

CREATE TABLE room_coverage (
    room_id TEXT PRIMARY KEY REFERENCES rooms(id),
    kind    TEXT NOT NULL CHECK (kind IN ('direct', 'human_direct', 'group')),
    created_at INTEGER NOT NULL,
    -- kind-specific 结构见证：每个已提交 Room 必须自证结构闭合。
    -- direct：唯一 Human–Agent 配对（PK/UNIQUE 保证至多一个）；
    -- human_direct：唯一 Human–Human 配对；group：唯一的 active owner 周期
    -- （group_owner_membership_id 必须指向 role='owner' 且
    -- active_marker=1 的 Membership 周期；关闭 owner 而不在同一事务内
    -- 完成转让即违反 deferred 外键而无法 COMMIT）。
    direct_human_id   TEXT,
    direct_agent_id   TEXT,
    human_direct_a_id TEXT,
    human_direct_b_id TEXT,
    group_owner_membership_id TEXT,
    group_owner_role  TEXT NOT NULL DEFAULT 'owner' CHECK (group_owner_role = 'owner'),
    group_owner_active_marker INTEGER NOT NULL DEFAULT 1 CHECK (group_owner_active_marker = 1),
    -- 每个 kind 恰好填其所需见证字段，其余必须为 NULL。
    CHECK (
        (kind = 'direct' AND direct_human_id IS NOT NULL AND direct_agent_id IS NOT NULL
            AND human_direct_a_id IS NULL AND human_direct_b_id IS NULL
            AND group_owner_membership_id IS NULL)
        OR (kind = 'human_direct' AND human_direct_a_id IS NOT NULL
            AND human_direct_b_id IS NOT NULL
            AND direct_human_id IS NULL AND direct_agent_id IS NULL
            AND group_owner_membership_id IS NULL)
        OR (kind = 'group' AND group_owner_membership_id IS NOT NULL
            AND direct_human_id IS NULL AND direct_agent_id IS NULL
            AND human_direct_a_id IS NULL AND human_direct_b_id IS NULL)
    ),
    -- human_direct 见证必须使用规范顺序（与 human_direct_pairs 一致）。
    CHECK (human_direct_a_id IS NULL OR human_direct_b_id IS NULL
        OR human_direct_a_id < human_direct_b_id),
    -- COMMIT 期闭合：覆盖行反向要求对应配对 / owner Membership 周期真实
    -- 存在且角色/active 状态一致（DEFERRABLE INITIALLY DEFERRED，允许
    -- 事务中间态暂时不完整）。SQLite 触发器只在 INSERT/UPDATE 阶段执行、
    -- 不能在 COMMIT 执行，COMMIT 期完整性只能由 deferred 外键证明。
    FOREIGN KEY (room_id, direct_human_id, direct_agent_id)
        REFERENCES direct_room_pairs(room_id, human_id, agent_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, human_direct_a_id, human_direct_b_id)
        REFERENCES human_direct_pairs(room_id, human_a_id, human_b_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, group_owner_membership_id, group_owner_role, group_owner_active_marker)
        REFERENCES room_memberships(room_id, membership_id, role, active_marker)
        DEFERRABLE INITIALLY DEFERRED
);

-- 覆盖 kind 必须等于 Room 的 kind；每个 Room 恰好一条覆盖行（主键）。
CREATE TRIGGER room_coverage_kind_matches_room
BEFORE INSERT ON room_coverage
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.kind = NEW.kind)
BEGIN
    SELECT RAISE(ABORT, 'room coverage kind must match the room kind');
END;

-- 覆盖行不可改写与 transfer 例外：kind、created_at 与
-- direct/human_direct 结构见证身份不可改写；group 覆盖行的
-- group_owner_membership_id 只能更新为同 room 的另一 active owner 周期
-- （transfer 事务内更新），其余 group 见证常量不可改写。deferred 复合
-- 外键在 COMMIT 时反向证明新见证是真实存在的 active owner 周期；直接
-- SQL 改写为不存在/非 owner/closed 的周期都会在 COMMIT 失败。
CREATE TRIGGER room_coverage_identity_immutable
BEFORE UPDATE OF room_id, kind, created_at, direct_human_id, direct_agent_id,
    human_direct_a_id, human_direct_b_id, group_owner_membership_id,
    group_owner_role, group_owner_active_marker
ON room_coverage
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.kind IS NOT OLD.kind
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.direct_human_id IS NOT OLD.direct_human_id
  OR NEW.direct_agent_id IS NOT OLD.direct_agent_id
  OR NEW.human_direct_a_id IS NOT OLD.human_direct_a_id
  OR NEW.human_direct_b_id IS NOT OLD.human_direct_b_id
  OR NEW.group_owner_role IS NOT OLD.group_owner_role
  OR NEW.group_owner_active_marker IS NOT OLD.group_owner_active_marker
  OR (OLD.kind <> 'group' AND NEW.group_owner_membership_id IS NOT OLD.group_owner_membership_id)
  OR (OLD.kind = 'group' AND NEW.group_owner_membership_id IS NOT OLD.group_owner_membership_id
      AND NOT EXISTS (
          SELECT 1 FROM room_memberships m
          WHERE m.room_id = NEW.room_id AND m.membership_id = NEW.group_owner_membership_id
            AND m.role = 'owner' AND m.active_marker = 1
      ))
BEGIN
    SELECT RAISE(ABORT, 'room coverage is immutable');
END;

CREATE TRIGGER room_coverage_no_delete
BEFORE DELETE ON room_coverage
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room coverage cannot be deleted');
END;

-- 身份防改写：REPLACE 以 room_coverage 主键 (room_id) 冲突时先隐式删除
-- 再重建，可绕过 no-delete / immutable 守卫改写 created_at 或结构见证
-- 身份（每个 Room 恰好一条覆盖行，覆盖行不可改写、不可硬删除）；本守卫
-- 在 INSERT 阶段直接拒绝 room_id 已存在的写入。
CREATE TRIGGER room_coverage_insert_identity_guard
BEFORE INSERT ON room_coverage
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM room_coverage WHERE room_id = NEW.room_id)
BEGIN
    SELECT RAISE(ABORT, 'room coverage identity already exists');
END;

CREATE TABLE direct_room_pairs (
    room_id     TEXT NOT NULL REFERENCES room_coverage(room_id),
    human_id    TEXT NOT NULL REFERENCES participants(id),
    agent_id    TEXT NOT NULL REFERENCES participants(id),
    human_membership_id TEXT NOT NULL,
    agent_membership_id TEXT NOT NULL,
    role_owner  TEXT NOT NULL DEFAULT 'owner' CHECK (role_owner = 'owner'),
    role_member TEXT NOT NULL DEFAULT 'member' CHECK (role_member = 'member'),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id),
    UNIQUE (room_id, human_id, agent_id),
    CHECK (human_id <> agent_id),
    FOREIGN KEY (room_id, human_membership_id, role_owner)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, agent_membership_id, role_member)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED
);

-- 覆盖结构：direct 配对只允许指向 kind=direct 的 Room。
CREATE TRIGGER direct_room_pairs_insert_kind_guard
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM room_coverage c
                 WHERE c.room_id = NEW.room_id AND c.kind = 'direct')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires a direct room coverage');
END;

-- 配对方向：human_id 必须指向 Human，agent_id 必须指向 Agent。
CREATE TRIGGER direct_room_pairs_insert_direction
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND kind = 'human')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND kind = 'agent')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair must be human -> agent');
END;

-- 配对双方必须是 active Participant。
CREATE TRIGGER direct_room_pairs_insert_active_participants
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_id AND status = 'active')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.agent_id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires active human and active agent');
END;

-- 创建时当前 Human 必须对该 Agent 存在 active Agent Owner 关系；
-- Agent Owner 不自动产生 Room，Room 必须显式创建。
CREATE TRIGGER direct_room_pairs_insert_active_owner
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM agent_owners
                 WHERE agent_id = NEW.agent_id AND human_id = NEW.human_id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'direct room pair requires an active agent owner');
END;

-- 配对身份（Room、Human、Agent、奠基 Membership、创建时间）一经创建
-- 不可改写。
CREATE TRIGGER direct_room_pairs_identity_immutable
BEFORE UPDATE OF room_id, human_id, agent_id, human_membership_id,
    agent_membership_id, created_at ON direct_room_pairs
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.human_id IS NOT OLD.human_id
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.human_membership_id IS NOT OLD.human_membership_id
  OR NEW.agent_membership_id IS NOT OLD.agent_membership_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'direct room pair is immutable');
END;

CREATE TRIGGER direct_room_pairs_no_delete
BEFORE DELETE ON direct_room_pairs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'direct room pairs cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id) 冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫把配对改挂到另一 Room 或改写 Human–Agent 对；
-- 本守卫在 INSERT 阶段直接拒绝 room_id 已占用的写入（同一 Human–Agent 对
-- 允许一房一条配对、多房并存，不再按 (human_id, agent_id) 唯一）。
CREATE TRIGGER direct_room_pairs_insert_identity_guard
BEFORE INSERT ON direct_room_pairs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_pairs
    WHERE room_id = NEW.room_id
)
BEGIN
    SELECT RAISE(ABORT, 'direct room pair identity already exists');
END;

CREATE TABLE human_direct_pairs (
    room_id     TEXT NOT NULL REFERENCES room_coverage(room_id),
    human_a_id  TEXT NOT NULL REFERENCES participants(id),
    human_b_id  TEXT NOT NULL REFERENCES participants(id),
    human_a_membership_id TEXT NOT NULL,
    human_b_membership_id TEXT NOT NULL,
    role_a      TEXT NOT NULL CHECK (role_a IN ('owner', 'member')),
    role_b      TEXT NOT NULL CHECK (role_b IN ('owner', 'member')),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (room_id),
    UNIQUE (human_a_id, human_b_id),
    UNIQUE (room_id, human_a_id, human_b_id),
    CHECK (human_a_id < human_b_id),
    CHECK (role_a <> role_b),
    CHECK ((role_a = 'owner' AND role_b = 'member')
        OR (role_a = 'member' AND role_b = 'owner')),
    FOREIGN KEY (human_a_id, human_b_id) REFERENCES contacts(a_id, b_id),
    FOREIGN KEY (room_id, human_a_membership_id, role_a)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, human_b_membership_id, role_b)
        REFERENCES room_memberships(room_id, membership_id, role)
        DEFERRABLE INITIALLY DEFERRED
);

-- 覆盖结构：human_direct 配对只允许指向 kind=human_direct 的 Room。
CREATE TRIGGER human_direct_pairs_insert_kind_guard
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM room_coverage c
                 WHERE c.room_id = NEW.room_id AND c.kind = 'human_direct')
BEGIN
    SELECT RAISE(ABORT, 'human direct pair requires a human_direct room coverage');
END;

-- 配对双方必须是 active Human。
CREATE TRIGGER human_direct_pairs_insert_direction
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_a_id AND kind = 'human' AND status = 'active')
  OR NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.human_b_id AND kind = 'human' AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'human direct pair requires two active humans');
END;

-- 创建时两端必须存在 accepted 联系人关系（复合外键已证明关系真实存在，
-- 此处证明关系当前 accepted）；pending 请求不能创建 Human direct Room。
CREATE TRIGGER human_direct_pairs_insert_accepted_contact
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM contacts
                 WHERE a_id = NEW.human_a_id AND b_id = NEW.human_b_id AND status = 'accepted')
BEGIN
    SELECT RAISE(ABORT, 'human direct room requires an accepted contact');
END;

-- 配对身份（Room、两端 Human、奠基 Membership）与创建时间一经创建不可改写。
CREATE TRIGGER human_direct_pairs_identity_immutable
BEFORE UPDATE OF room_id, human_a_id, human_b_id, human_a_membership_id,
    human_b_membership_id, created_at ON human_direct_pairs
FOR EACH ROW
WHEN NEW.room_id IS NOT OLD.room_id
  OR NEW.human_a_id IS NOT OLD.human_a_id
  OR NEW.human_b_id IS NOT OLD.human_b_id
  OR NEW.human_a_membership_id IS NOT OLD.human_a_membership_id
  OR NEW.human_b_membership_id IS NOT OLD.human_b_membership_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'human direct pair is immutable');
END;

CREATE TRIGGER human_direct_pairs_no_delete
BEFORE DELETE ON human_direct_pairs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'human direct pairs cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (room_id) 或 UNIQUE (human_a_id, human_b_id)
-- 冲突时先隐式删除冲突行，可绕过 no-delete / immutable 守卫改写配对或
-- Room；本守卫在 INSERT 阶段直接拒绝任一身份已占用的写入。
CREATE TRIGGER human_direct_pairs_insert_identity_guard
BEFORE INSERT ON human_direct_pairs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM human_direct_pairs
    WHERE room_id = NEW.room_id
       OR (human_a_id = NEW.human_a_id AND human_b_id = NEW.human_b_id)
)
BEGIN
    SELECT RAISE(ABORT, 'human direct pair identity already exists');
END;

CREATE TABLE room_memberships (
    membership_id        TEXT PRIMARY KEY,
    room_id              TEXT NOT NULL REFERENCES rooms(id),
    participant_id       TEXT NOT NULL REFERENCES participants(id),
    role                 TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    joined_at            INTEGER NOT NULL,
    join_after_sequence  INTEGER NOT NULL CHECK (join_after_sequence >= 0),
    left_at              INTEGER,
    left_before_sequence INTEGER,
    closed_by            TEXT,
    active_marker        INTEGER NOT NULL DEFAULT 1 CHECK (active_marker IN (0, 1)),
    CHECK (typeof(membership_id) = 'text'),
    CHECK (length(membership_id) = 36),
    CHECK (substr(membership_id, 9, 1) = '-'),
    CHECK (substr(membership_id, 14, 1) = '-'),
    CHECK (substr(membership_id, 19, 1) = '-'),
    CHECK (substr(membership_id, 24, 1) = '-'),
    CHECK (substr(membership_id, 15, 1) = '4'),
    CHECK (substr(membership_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (membership_id = lower(membership_id)),
    CHECK (instr(membership_id, char(0)) = 0),
    CHECK (membership_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(membership_id, '-', '')) = 32),
    -- active_marker 与 left 三元组保持一致：active 周期三个 left 字段全
    -- NULL；closed 周期三个字段全非 NULL 且 closed_by 为合法原因。
    CHECK ((active_marker = 1 AND left_at IS NULL AND left_before_sequence IS NULL AND closed_by IS NULL)
        OR (active_marker = 0 AND left_at IS NOT NULL AND left_before_sequence IS NOT NULL
            AND closed_by IS NOT NULL
            AND closed_by IN ('leave', 'removed', 'transfer', 'agent_revoked'))),
    CHECK (left_before_sequence IS NULL OR left_before_sequence >= join_after_sequence),
    -- coverage / 配对复合外键的 parent keys。
    UNIQUE (room_id, membership_id, role),
    UNIQUE (room_id, membership_id, role, active_marker),
    -- group_owner_transitions 复合外键的 parent keys：转让事实必须以精确
    -- 的周期字段见证四条同 Room Membership（角色/participant/时间/边界/
    -- 关闭原因全部匹配；membership_id 是主键，这些 UNIQUE 恒唯一，只为
    -- FK parent key 存在）。
    UNIQUE (room_id, membership_id, participant_id, role, left_at,
        left_before_sequence, closed_by),
    UNIQUE (room_id, membership_id, participant_id, role, joined_at,
        join_after_sequence),
    -- 幂等 witness 复合外键的 parent key：group_member_invite_idempotency
    -- 的 (room_id, participant_id, membership_id) 与
    -- group_owner_transfer_idempotency 的 (room_id, target_participant_id,
    -- new_owner_membership_id) 都必须对应同一条真实 Membership 周期行
    -- （不能只引用 membership_id 而让 room/participant 指向另一事实）。
    UNIQUE (room_id, participant_id, membership_id)
);

-- 同 Room+Participant 至多一个 active 周期（部分唯一索引，仅约束
-- active_marker=1；closed 历史周期不占位，可累积多条：leave → 重入 →
-- 再 leave 都是新 closed 周期）。REPLACE 的隐式删除发生在 BEFORE INSERT
-- 触发器之后，身份守卫在第一次触发时即看到既有 active 周期并拒绝改写。
CREATE UNIQUE INDEX ux_room_memberships_single_active_period
    ON room_memberships(room_id, participant_id)
    WHERE active_marker = 1;

-- 每 Room 至多一个 active owner：部分唯一索引在数据库层面自证，不允许
-- 任何（应用或直接 SQL）写入产生第二个 active owner 周期；转让事务必须先
-- 关闭旧 owner 再打开新 owner。
CREATE UNIQUE INDEX ux_room_memberships_single_active_owner
    ON room_memberships(room_id)
    WHERE role = 'owner' AND active_marker = 1;

CREATE INDEX idx_room_memberships_participant ON room_memberships(participant_id);
CREATE INDEX idx_room_memberships_room_active ON room_memberships(room_id, active_marker);

-- Membership 结构守卫：direct 只允许配对中的奠基 Human owner 与 Agent
-- member 周期（membership_id 必须正是配对引用的奠基周期，participant 与
-- role 必须与配对一致）；human_direct 只允许配对两端 Human 各自固定的
-- role（同样以 membership_id 锚定）；group 接受任意成员（单 active owner
-- 与成员资格由后续守卫裁决）。
CREATE TRIGGER room_memberships_insert_structure_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.kind = 'group'
)
AND NOT EXISTS (
    SELECT 1 FROM direct_room_pairs p
    WHERE p.room_id = NEW.room_id
      AND ((NEW.membership_id = p.human_membership_id AND NEW.participant_id = p.human_id
            AND NEW.role = 'owner')
        OR (NEW.membership_id = p.agent_membership_id AND NEW.participant_id = p.agent_id
            AND NEW.role = 'member'))
)
AND NOT EXISTS (
    SELECT 1 FROM human_direct_pairs h
    WHERE h.room_id = NEW.room_id
      AND ((NEW.membership_id = h.human_a_membership_id AND NEW.participant_id = h.human_a_id
            AND NEW.role = h.role_a)
        OR (NEW.membership_id = h.human_b_membership_id AND NEW.participant_id = h.human_b_id
            AND NEW.role = h.role_b))
)
BEGIN
    SELECT RAISE(ABORT, 'membership does not match the room structure');
END;

-- active Membership 周期只能指向 active Participant（Room 恒 active）。
CREATE TRIGGER room_memberships_active_requires_active_participant_insert
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker = 1
  AND NOT EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND status = 'active')
BEGIN
    SELECT RAISE(ABORT, 'active membership requires active participant');
END;

-- 任何 INSERT 只能创建 active 周期（active_marker=1 且 left 三元组全
-- NULL；SQLite 无列级 INSERT CHECK）；closed 周期只能由 UPDATE 关闭路径
-- 产生，不能直接插入，也不能经 REPLACE 复活。
CREATE TRIGGER room_memberships_insert_active_only
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker <> 1
  OR NEW.left_at IS NOT NULL
  OR NEW.left_before_sequence IS NOT NULL
  OR NEW.closed_by IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'new room memberships must be active periods with a null left tuple');
END;

-- Membership 周期身份（membership_id、Room、Participant、role、joined_at、
-- join_after_sequence）不可改写。
CREATE TRIGGER room_memberships_identity_immutable
BEFORE UPDATE OF membership_id, room_id, participant_id, role, joined_at,
    join_after_sequence ON room_memberships
FOR EACH ROW
WHEN NEW.membership_id IS NOT OLD.membership_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.participant_id IS NOT OLD.participant_id
  OR NEW.role IS NOT OLD.role
  OR NEW.joined_at IS NOT OLD.joined_at
  OR NEW.join_after_sequence IS NOT OLD.join_after_sequence
BEGIN
    SELECT RAISE(ABORT, 'membership identity is immutable');
END;

-- active 周期必须保持 left 三元组全 NULL（不允许半关闭状态）。
CREATE TRIGGER room_memberships_active_left_consistency
BEFORE UPDATE OF left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN NEW.active_marker = 1
  AND (NEW.left_at IS NOT NULL OR NEW.left_before_sequence IS NOT NULL OR NEW.closed_by IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'active membership must keep a null left tuple');
END;

-- 关闭周期必须一次写满 left 三元组且 closed_by 为合法原因。
CREATE TRIGGER room_memberships_close_requires_full_tuple
BEFORE UPDATE OF active_marker, left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND (NEW.left_at IS NULL OR NEW.left_before_sequence IS NULL OR NEW.closed_by IS NULL
       OR NEW.closed_by NOT IN ('leave', 'removed', 'transfer', 'agent_revoked'))
BEGIN
    SELECT RAISE(ABORT, 'closing a membership requires left_at, left_before_sequence and a valid closed_by');
END;

-- closed 周期是终态：整行不可再改写（含 reopened 为 active）。
CREATE TRIGGER room_memberships_closed_immutable
BEFORE UPDATE OF active_marker, left_at, left_before_sequence, closed_by ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 0
  AND (NEW.active_marker IS NOT OLD.active_marker
       OR NEW.left_at IS NOT OLD.left_at
       OR NEW.left_before_sequence IS NOT OLD.left_before_sequence
       OR NEW.closed_by IS NOT OLD.closed_by)
BEGIN
    SELECT RAISE(ABORT, 'closed membership periods are immutable');
END;

-- Human 周期关闭守卫：direct/human_direct 的 Human 周期永远 active；
-- group 的普通 member 周期可以由 removed（owner 移除）、leave（主动
-- 退群）或 transfer（群主转让）关闭；group 的 owner 周期只能经合法
-- transfer 关闭——closed_by='leave'/'removed'/'agent_revoked' 直接被
-- 拒绝，'transfer' 必须在同一事务内完成目标 member 关闭、新 owner 打开
-- 与 coverage 见证更新（否则 COMMIT 期 deferred 复合外键失败）。
CREATE TRIGGER room_memberships_human_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND kind = 'human')
  AND NOT (
    (NEW.role = 'member'
     AND NEW.closed_by IN ('removed', 'leave', 'transfer')
     AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
    OR
    (NEW.role = 'owner'
     AND NEW.closed_by = 'transfer'
     AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
  )
BEGIN
    SELECT RAISE(ABORT, 'human membership can only close as a group member leave/removal or via owner transfer');
END;

-- Agent 周期关闭守卫：只有两种合法路径——(a) 对应 Agent Participant 已
-- revoked 且 left_at 等于该 Agent 的 revoked_at（Agent revoke 传播触发器
-- 在同一事务内用 NEW.revoked_at 完成，覆盖 direct/human_direct/group 全部
-- 房间）；(b) group 的成员周期由 removed（owner 移除）或 transfer（群主
-- 转让）关闭。直接关闭 active Agent 的 direct/human_direct 周期被拒绝。
CREATE TRIGGER room_memberships_agent_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0
  AND EXISTS (SELECT 1 FROM participants WHERE id = NEW.participant_id AND kind = 'agent')
  AND NOT (
    (NEW.closed_by = 'agent_revoked'
     AND EXISTS (SELECT 1 FROM participants
                 WHERE id = NEW.participant_id AND status = 'revoked' AND revoked_at = NEW.left_at))
    OR (NEW.closed_by IN ('removed', 'transfer')
        AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group'))
  )
BEGIN
    SELECT RAISE(ABORT, 'agent membership close requires agent revoke propagation or a group member removal');
END;

-- transfer 关闭守卫（拆分防护）：任何 active 周期以 closed_by='transfer'
-- 关闭（Human 的旧 owner / 目标 member，Agent 周期同受约束——Agent 周期
-- 不可能被任何 transition 见证，因此 Agent 的 transfer 关闭永远被拒，只能
-- 走 removed）时，当前事务中必须已存在匹配的 group_owner_transitions
-- 事实：被关闭的周期正是该 transition 的 old_owner_membership_id 或
-- target_member_membership_id，且 room_id、occurred_at=NEW.left_at、
-- boundary_sequence=NEW.left_before_sequence 全部相同。这样 transfer 关闭
-- 不可能拆成独立事务执行：T1 单独关闭目标 member（或旧 owner）会立即
-- 失败；合法转让事务先插 transition 再关闭所以通过。时间/边界错配（关闭
-- 值与 transition 的 occurred_at/boundary_sequence 不一致）同样在此被拒。
-- direct/human_direct 的 transfer 关闭已由 human/agent close guard 拒绝；
-- transition 只属于 group，本守卫进一步把 transfer 关闭钉死在 group 的
-- 完整转让事务内。
CREATE TRIGGER room_memberships_transfer_close_guard
BEFORE UPDATE OF active_marker ON room_memberships
FOR EACH ROW
WHEN OLD.active_marker = 1 AND NEW.active_marker = 0 AND NEW.closed_by = 'transfer'
  AND NOT EXISTS (
      SELECT 1 FROM group_owner_transitions t
      WHERE t.room_id = NEW.room_id
        AND (t.old_owner_membership_id = NEW.membership_id
             OR t.target_member_membership_id = NEW.membership_id)
        AND t.occurred_at = NEW.left_at
        AND t.boundary_sequence = NEW.left_before_sequence
  )
BEGIN
    SELECT RAISE(ABORT, 'a transfer close requires a same-transaction matching owner transition');
END;

-- group 恰好一个 active owner：插入第二个 active owner 周期被拒（第一个
-- owner 周期即创建者；owner 变更只能经 transfer 事务关闭旧周期再打开新
-- 周期，部分唯一索引在数据库层面兜底）。
CREATE TRIGGER room_memberships_group_single_owner
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.role = 'owner'
  AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND EXISTS (SELECT 1 FROM room_memberships m
              WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1)
BEGIN
    SELECT RAISE(ABORT, 'a group already has an active owner');
END;

-- group owner 周期必须有精确来路：要么是该 group 的首个 owner（该 Room
-- 尚无任何 Membership 周期——创建者奠基），要么是同一事务内由
-- group_owner_transitions 事实精确见证的转让目标（NEW 的
-- membership_id/room/participant/joined_at/join_after_sequence 与某条
-- transition 的 new_owner_membership_id/room_id/target_participant_id/
-- occurred_at/boundary_sequence 完全一致；transition 可先插入、其新周期
-- FK deferred 引用随后插入的新周期）。不看模糊历史：某人历史上有任一
-- transfer 关闭周期这一事实本身不能让他再次成为 owner——已被真实 SQLite
-- 反向实验绕过过（转让后 leave、无 active 周期仍可被复用为 owner），
-- 因此本守卫只接受「无任何周期」或「同一事务内精确 transition」两种来路，
-- 从未入群的 Human 或离群后再插的 Human 直接插成 active owner 再更新
-- coverage 提交被拒绝。
CREATE TRIGGER room_memberships_group_owner_transition_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN NEW.role = 'owner'
  AND EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND NOT (
    NOT EXISTS (SELECT 1 FROM room_memberships m
                WHERE m.room_id = NEW.room_id)
    OR
    EXISTS (SELECT 1 FROM group_owner_transitions t
            WHERE t.room_id = NEW.room_id
              AND t.target_participant_id = NEW.participant_id
              AND t.new_owner_membership_id = NEW.membership_id
              AND t.occurred_at = NEW.joined_at
              AND t.boundary_sequence = NEW.join_after_sequence)
  )
BEGIN
    SELECT RAISE(ABORT, 'group owner must be the founding owner or a same-transaction transfer target');
END;

-- group 成员资格守卫（owner 插入与成员邀请共用）：第一个 owner 必须是
-- active Human；member 必须是 group active owner 的 accepted 联系人
-- （Human）或自己拥有且 active 的 Agent。邀请/转让操作在同一事务内完成
-- owner 权限检查、目标资格检查和 Membership 周期创建；不存在「已邀请但
-- 尚未成员」的中间状态。
CREATE TRIGGER room_memberships_group_member_qualification
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM rooms WHERE id = NEW.room_id AND kind = 'group')
  AND NOT (
    (NEW.role = 'owner'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'human' AND p.status = 'active')
     AND NOT EXISTS (SELECT 1 FROM room_memberships m
                     WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'human' AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM room_memberships o
                 JOIN contacts c ON ((c.a_id = o.participant_id AND c.b_id = NEW.participant_id)
                                   OR (c.a_id = NEW.participant_id AND c.b_id = o.participant_id))
                 WHERE o.room_id = NEW.room_id AND o.role = 'owner' AND o.active_marker = 1
                   AND c.status = 'accepted'))
    OR
    (NEW.role = 'member'
     AND EXISTS (SELECT 1 FROM participants p
                 WHERE p.id = NEW.participant_id AND p.kind = 'agent' AND p.status = 'active')
     AND EXISTS (SELECT 1 FROM room_memberships o
                 JOIN agent_owners ao ON ao.agent_id = NEW.participant_id AND ao.human_id = o.participant_id
                 WHERE o.room_id = NEW.room_id AND o.role = 'owner' AND o.active_marker = 1
                   AND ao.status = 'active'))
  )
BEGIN
    SELECT RAISE(ABORT, 'group membership requires the owner and an accepted human or owned active agent');
END;

-- Membership 周期不允许硬删除（删除即违反该 Room 的配对/覆盖完整性）。
CREATE TRIGGER room_memberships_no_delete
BEFORE DELETE ON room_memberships
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'room memberships cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (membership_id) 或部分唯一索引 (room_id,
-- participant_id WHERE active_marker=1) 冲突时先隐式删除冲突行，可绕过
-- no-delete / immutable 守卫把 closed 周期复活为 active 或改写 role /
-- 序列边界；BEFORE INSERT 触发器先于 REPLACE 的隐式删除执行，本守卫在
-- 第一次触发时即拒绝任一身份已占用的写入（closed 是终态，不能经 INSERT
-- 恢复；同 Room+Participant 至多一个 active 周期，不能用 REPLACE 换掉
-- 既有 active 周期）。
CREATE TRIGGER room_memberships_insert_identity_guard
BEFORE INSERT ON room_memberships
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM room_memberships
    WHERE membership_id = NEW.membership_id
       OR (room_id = NEW.room_id AND participant_id = NEW.participant_id
           AND NEW.active_marker = 1 AND active_marker = 1)
)
BEGIN
    SELECT RAISE(ABORT, 'room membership identity already exists');
END;

-- 群主转让事实表：每次 owner 转让由一条不可变 transition 精确关联本次
-- 关闭/打开的四条同 Room Membership 周期与同一 sequence 边界。四条周期
-- 以 DEFERRABLE 复合外键（parent keys：room_memberships 的 UNIQUE
-- (room_id, membership_id, participant_id, role, left_at,
-- left_before_sequence, closed_by) 与 UNIQUE (room_id, membership_id,
-- participant_id, role, joined_at, join_after_sequence)）反向证明：
-- 旧 owner 与目标 member 周期必须是同 Room、同 role、同 occurred_at /
-- boundary 以 closed_by='transfer' 关闭的真实行；新 owner 与旧 owner 新
-- member 周期必须是同 Room、同 role、joined_at=occurred_at、
-- join_after=boundary 的真实行。新周期 FK 不绑定 active_marker——它们
-- 之后可被下一次合法转让关闭（先作为某次 new_owner、之后作为下一次
-- old_owner 是不同角色列，允许）。角色/关闭原因列用 DEFAULT+CHECK 写死
-- 常量，防止见证错位。每条周期在同一角色列 UNIQUE：同一周期不能在同一
-- 种角色列被第二次消费（历史 target member / old owner 周期不能被重复
-- 复用），跨角色列（new_owner → 下次 old_owner、old_owner_new_member →
-- 下次 target_member）合法。old_owner != target、四条周期 ID 两两不同、
-- boundary_sequence >= 0 由 CHECK 自证。
CREATE TABLE group_owner_transitions (
    transition_id TEXT PRIMARY KEY,
    room_id       TEXT NOT NULL REFERENCES rooms(id),
    old_owner_participant_id TEXT NOT NULL REFERENCES participants(id),
    target_participant_id    TEXT NOT NULL REFERENCES participants(id),
    old_owner_membership_id         TEXT NOT NULL,
    target_member_membership_id     TEXT NOT NULL,
    new_owner_membership_id         TEXT NOT NULL,
    old_owner_new_member_membership_id TEXT NOT NULL,
    occurred_at         INTEGER NOT NULL,
    boundary_sequence   INTEGER NOT NULL CHECK (boundary_sequence >= 0),
    old_owner_role           TEXT NOT NULL DEFAULT 'owner' CHECK (old_owner_role = 'owner'),
    old_owner_closed_by      TEXT NOT NULL DEFAULT 'transfer' CHECK (old_owner_closed_by = 'transfer'),
    target_member_role       TEXT NOT NULL DEFAULT 'member' CHECK (target_member_role = 'member'),
    target_member_closed_by  TEXT NOT NULL DEFAULT 'transfer' CHECK (target_member_closed_by = 'transfer'),
    new_owner_role           TEXT NOT NULL DEFAULT 'owner' CHECK (new_owner_role = 'owner'),
    old_owner_new_member_role TEXT NOT NULL DEFAULT 'member' CHECK (old_owner_new_member_role = 'member'),
    CHECK (typeof(transition_id) = 'text'),
    CHECK (length(transition_id) = 36),
    CHECK (substr(transition_id, 9, 1) = '-'),
    CHECK (substr(transition_id, 14, 1) = '-'),
    CHECK (substr(transition_id, 19, 1) = '-'),
    CHECK (substr(transition_id, 24, 1) = '-'),
    CHECK (substr(transition_id, 15, 1) = '4'),
    CHECK (substr(transition_id, 20, 1) IN ('8', '9', 'a', 'b')),
    CHECK (transition_id = lower(transition_id)),
    CHECK (instr(transition_id, char(0)) = 0),
    CHECK (transition_id NOT GLOB '*[^0-9a-f-]*'),
    CHECK (length(replace(transition_id, '-', '')) = 32),
    CHECK (old_owner_participant_id <> target_participant_id),
    CHECK (old_owner_membership_id <> target_member_membership_id),
    CHECK (old_owner_membership_id <> new_owner_membership_id),
    CHECK (old_owner_membership_id <> old_owner_new_member_membership_id),
    CHECK (target_member_membership_id <> new_owner_membership_id),
    CHECK (target_member_membership_id <> old_owner_new_member_membership_id),
    CHECK (new_owner_membership_id <> old_owner_new_member_membership_id),
    -- 每条周期在同一角色列至多消费一次。
    UNIQUE (old_owner_membership_id),
    UNIQUE (target_member_membership_id),
    UNIQUE (new_owner_membership_id),
    UNIQUE (old_owner_new_member_membership_id),
    -- group_owner_transfer_idempotency 复合外键的 parent key：证明请求者
    -- 即旧 owner（(human_id, transition_id) → (old_owner_participant_id,
    -- transition_id)）。
    UNIQUE (old_owner_participant_id, transition_id),
    -- 四条同 Room Membership 周期见证（COMMIT 期由 deferred 外键裁决；
    -- transition 可先插入，新周期随后插入）。
    FOREIGN KEY (room_id, old_owner_membership_id, old_owner_participant_id,
                 old_owner_role, occurred_at, boundary_sequence, old_owner_closed_by)
        REFERENCES room_memberships(room_id, membership_id, participant_id, role,
            left_at, left_before_sequence, closed_by)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, target_member_membership_id, target_participant_id,
                 target_member_role, occurred_at, boundary_sequence, target_member_closed_by)
        REFERENCES room_memberships(room_id, membership_id, participant_id, role,
            left_at, left_before_sequence, closed_by)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, new_owner_membership_id, target_participant_id,
                 new_owner_role, occurred_at, boundary_sequence)
        REFERENCES room_memberships(room_id, membership_id, participant_id, role,
            joined_at, join_after_sequence)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (room_id, old_owner_new_member_membership_id,
                 old_owner_participant_id, old_owner_new_member_role,
                 occurred_at, boundary_sequence)
        REFERENCES room_memberships(room_id, membership_id, participant_id, role,
            joined_at, join_after_sequence)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX idx_group_owner_transitions_room
    ON group_owner_transitions(room_id);
CREATE INDEX idx_group_owner_transitions_target
    ON group_owner_transitions(target_participant_id);

-- transition 整行不可改写：任何列一经创建固定。
CREATE TRIGGER group_owner_transitions_immutable
BEFORE UPDATE OF transition_id, room_id, old_owner_participant_id,
    target_participant_id, old_owner_membership_id,
    target_member_membership_id, new_owner_membership_id,
    old_owner_new_member_membership_id, occurred_at, boundary_sequence,
    old_owner_role, old_owner_closed_by, target_member_role,
    target_member_closed_by, new_owner_role, old_owner_new_member_role
ON group_owner_transitions
FOR EACH ROW
WHEN NEW.transition_id IS NOT OLD.transition_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.old_owner_participant_id IS NOT OLD.old_owner_participant_id
  OR NEW.target_participant_id IS NOT OLD.target_participant_id
  OR NEW.old_owner_membership_id IS NOT OLD.old_owner_membership_id
  OR NEW.target_member_membership_id IS NOT OLD.target_member_membership_id
  OR NEW.new_owner_membership_id IS NOT OLD.new_owner_membership_id
  OR NEW.old_owner_new_member_membership_id IS NOT OLD.old_owner_new_member_membership_id
  OR NEW.occurred_at IS NOT OLD.occurred_at
  OR NEW.boundary_sequence IS NOT OLD.boundary_sequence
  OR NEW.old_owner_role IS NOT OLD.old_owner_role
  OR NEW.old_owner_closed_by IS NOT OLD.old_owner_closed_by
  OR NEW.target_member_role IS NOT OLD.target_member_role
  OR NEW.target_member_closed_by IS NOT OLD.target_member_closed_by
  OR NEW.new_owner_role IS NOT OLD.new_owner_role
  OR NEW.old_owner_new_member_role IS NOT OLD.old_owner_new_member_role
BEGIN
    SELECT RAISE(ABORT, 'group owner transitions are immutable');
END;

-- transition 不允许硬删除。
CREATE TRIGGER group_owner_transitions_no_delete
BEFORE DELETE ON group_owner_transitions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'group owner transitions cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (transition_id) 冲突时先隐式删除冲突行，
-- 可绕过 no-delete / immutable 守卫重建转让事实；本守卫在 INSERT 阶段
-- 直接拒绝 transition_id 已存在的写入。
CREATE TRIGGER group_owner_transitions_insert_identity_guard
BEFORE INSERT ON group_owner_transitions
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM group_owner_transitions
             WHERE transition_id = NEW.transition_id)
BEGIN
    SELECT RAISE(ABORT, 'group owner transition identity already exists');
END;

-- transition INSERT guard（拆分防护）：转让事实必须在旧 owner 与目标
-- member 周期仍 active 时插入——即先于任何 transfer 关闭，由它在当前
-- 事务中为之后的关闭提供见证。写入时必须当场证明：Room 是 group；
-- old_owner_membership_id 是当前 active、同 room、participant=旧 owner、
-- role=owner 的周期；target_member_membership_id 是当前 active、同 room、
-- participant=目标、role=member 的周期；两端 participant 是 active Human；
-- 且 old owner 是该 Room 当前唯一 active owner（除旧 owner 周期外没有
-- 其他 active owner）。合法性不依赖 deferred FK 的 COMMIT 期裁决：旧
-- owner 或目标已关闭、周期错位（room/participant/role 不匹配）、非 group
-- Room、非 active Human 的 transition 在写入时就被拒，任何已关闭的历史
-- 周期都不可能被放进角色列重复消费（closed 是终态，不会重新 active）。
-- 四条周期的 deferred 复合外键仍保留，作为「关闭/打开值必须精确匹配
-- transition 列」的 COMMIT 期兜底。
CREATE TRIGGER group_owner_transitions_insert_guard
BEFORE INSERT ON group_owner_transitions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r WHERE r.id = NEW.room_id AND r.kind = 'group'
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.membership_id = NEW.old_owner_membership_id
      AND m.room_id = NEW.room_id
      AND m.participant_id = NEW.old_owner_participant_id
      AND m.role = 'owner'
      AND m.active_marker = 1
)
OR NOT EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.membership_id = NEW.target_member_membership_id
      AND m.room_id = NEW.room_id
      AND m.participant_id = NEW.target_participant_id
      AND m.role = 'member'
      AND m.active_marker = 1
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.old_owner_participant_id AND p.kind = 'human' AND p.status = 'active'
)
OR NOT EXISTS (
    SELECT 1 FROM participants p
    WHERE p.id = NEW.target_participant_id AND p.kind = 'human' AND p.status = 'active'
)
OR EXISTS (
    SELECT 1 FROM room_memberships m
    WHERE m.room_id = NEW.room_id AND m.role = 'owner' AND m.active_marker = 1
      AND m.membership_id <> NEW.old_owner_membership_id
)
BEGIN
    SELECT RAISE(ABORT, 'group owner transition requires an active human old owner and an active human member target in the same group room');
END;

CREATE TABLE direct_room_idempotency (
    human_id       TEXT NOT NULL REFERENCES participants(id),
    key            TEXT NOT NULL,
    agent_id       TEXT NOT NULL REFERENCES participants(id),
    room_id        TEXT NOT NULL REFERENCES rooms(id),
    name           TEXT,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    FOREIGN KEY (room_id, human_id, agent_id)
        REFERENCES direct_room_pairs(room_id, human_id, agent_id),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录必须见证请求的规范化标题（含 null）：name 写入时必须等于
-- rooms.name（room 与标题都不可变，写入时证明即终局证明；`IS` 保证
-- NULL 与 NULL 相等）。null 与非 null 标题是不同 payload 事实。
CREATE TRIGGER direct_room_idempotency_insert_name_witness
BEFORE INSERT ON direct_room_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms WHERE id = NEW.room_id AND name IS NEW.name
)
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency name must witness the room name');
END;

-- 幂等记录整行不可改写：human_id、key、agent_id、room_id、name、created_at
-- 任一列一经创建固定；等值自赋值不触发守卫。
CREATE TRIGGER direct_room_idempotency_immutable
BEFORE UPDATE OF human_id, key, agent_id, room_id, name, created_at
ON direct_room_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.agent_id IS NOT OLD.agent_id
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.name IS NOT OLD.name
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency records are immutable');
END;

CREATE TRIGGER direct_room_idempotency_no_delete
BEFORE DELETE ON direct_room_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行，
-- 可绕过 no-delete / immutable 守卫把同 key 重映射到另一 Room / 配对；
-- 本守卫在 INSERT 阶段直接拒绝 (human_id, key) 已存在的写入。
CREATE TRIGGER direct_room_idempotency_insert_identity_guard
BEFORE INSERT ON direct_room_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM direct_room_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'direct room idempotency identity already exists');
END;

CREATE TABLE human_direct_room_idempotency (
    human_id        TEXT NOT NULL REFERENCES participants(id),
    key             TEXT NOT NULL,
    peer_human_id   TEXT NOT NULL REFERENCES participants(id),
    canonical_a     TEXT NOT NULL REFERENCES participants(id),
    canonical_b     TEXT NOT NULL REFERENCES participants(id),
    room_id         TEXT NOT NULL REFERENCES rooms(id),
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    CHECK (canonical_a < canonical_b),
    CHECK ((canonical_a = human_id AND canonical_b = peer_human_id)
        OR (canonical_a = peer_human_id AND canonical_b = human_id)),
    FOREIGN KEY (room_id, canonical_a, canonical_b)
        REFERENCES human_direct_pairs(room_id, human_a_id, human_b_id),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录整行不可改写。
CREATE TRIGGER human_direct_room_idempotency_immutable
BEFORE UPDATE OF human_id, key, peer_human_id, canonical_a, canonical_b, room_id, created_at
ON human_direct_room_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.peer_human_id IS NOT OLD.peer_human_id
  OR NEW.canonical_a IS NOT OLD.canonical_a
  OR NEW.canonical_b IS NOT OLD.canonical_b
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'human direct room idempotency records are immutable');
END;

CREATE TRIGGER human_direct_room_idempotency_no_delete
BEFORE DELETE ON human_direct_room_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'human direct room idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行，
-- 可绕过 no-delete / immutable 守卫重映射 Room / 对端 Human；本守卫在
-- INSERT 阶段直接拒绝 (human_id, key) 已存在的写入。
CREATE TRIGGER human_direct_room_idempotency_insert_identity_guard
BEFORE INSERT ON human_direct_room_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM human_direct_room_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'human direct room idempotency identity already exists');
END;

CREATE TABLE group_create_idempotency (
    human_id   TEXT NOT NULL REFERENCES participants(id),
    key        TEXT NOT NULL,
    room_id    TEXT NOT NULL REFERENCES rooms(id),
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    CHECK (length(name) <= 80),
    CHECK (instr(name, char(0)) = 0),
    CHECK (trim(name, char(9) || char(10) || char(13) || ' ') <> ''),
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录整行不可改写。
CREATE TRIGGER group_create_idempotency_immutable
BEFORE UPDATE OF human_id, key, room_id, name, created_at
ON group_create_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.name IS NOT OLD.name
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'group create idempotency records are immutable');
END;

CREATE TRIGGER group_create_idempotency_no_delete
BEFORE DELETE ON group_create_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'group create idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行，
-- 可绕过 no-delete / immutable 守卫把同 key 重映射到另一 Room / 群名；
-- 本守卫在 INSERT 阶段直接拒绝 (human_id, key) 已存在的写入。
CREATE TRIGGER group_create_idempotency_insert_identity_guard
BEFORE INSERT ON group_create_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM group_create_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'group create idempotency identity already exists');
END;

-- 群邀请/重入幂等记录：human + key → room + participant + membership_id。
-- 每个成功 key（包括目标已有 active 周期而返回 200 的情况）都记录；
-- 同 key 重试即使该周期后来关闭也只能重放历史结果（membership_id 指向
-- 的周期行不可变），绝不能误开新周期；重新邀请必须使用新 key。
-- (room_id, participant_id, membership_id) 以 DEFERRABLE 复合外键对应
-- room_memberships 的同一条真实周期行（parent key：
-- UNIQUE (room_id, participant_id, membership_id)），写入时守卫再证明该
-- 周期当时是 group member 周期（不是 owner）；周期后来被关闭不影响历史
-- 幂等记录，旧 key 仍可重放历史周期。
CREATE TABLE group_member_invite_idempotency (
    human_id       TEXT NOT NULL REFERENCES participants(id),
    key            TEXT NOT NULL,
    room_id        TEXT NOT NULL REFERENCES rooms(id),
    participant_id TEXT NOT NULL REFERENCES participants(id),
    membership_id  TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    FOREIGN KEY (room_id, participant_id, membership_id)
        REFERENCES room_memberships(room_id, participant_id, membership_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 写入时守卫：幂等记录见证的必须是 group Room 中 role='member' 的周期
-- （owner 周期、非 group Room 周期、与其他 participant/room 错配的
-- membership 都在写入时被拒；三列与同一行的一致性由复合外键证明）。
-- 只检查写入时刻的角色，不要求周期永远 active——该周期后来被关闭不影响
-- 历史 key 的重放。
CREATE TRIGGER group_member_invite_idempotency_insert_membership_guard
BEFORE INSERT ON group_member_invite_idempotency
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM rooms r
    JOIN room_memberships m
      ON m.room_id = r.id
     AND m.participant_id = NEW.participant_id
     AND m.membership_id = NEW.membership_id
    WHERE r.id = NEW.room_id AND r.kind = 'group' AND m.role = 'member'
)
BEGIN
    SELECT RAISE(ABORT, 'invite idempotency must witness a same-row group member membership period');
END;

-- 幂等记录整行不可改写。
CREATE TRIGGER group_member_invite_idempotency_immutable
BEFORE UPDATE OF human_id, key, room_id, participant_id, membership_id, created_at
ON group_member_invite_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.room_id IS NOT OLD.room_id
  OR NEW.participant_id IS NOT OLD.participant_id
  OR NEW.membership_id IS NOT OLD.membership_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'group member invite idempotency records are immutable');
END;

CREATE TRIGGER group_member_invite_idempotency_no_delete
BEFORE DELETE ON group_member_invite_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'group member invite idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行，
-- 可绕过 no-delete / immutable 守卫把同 key 重映射到另一 Room /
-- participant / membership；本守卫在 INSERT 阶段直接拒绝 (human_id, key)
-- 已存在的写入。
CREATE TRIGGER group_member_invite_idempotency_insert_identity_guard
BEFORE INSERT ON group_member_invite_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM group_member_invite_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'group member invite idempotency identity already exists');
END;

-- 群主转让幂等记录：human + key → transition_id。转让事务在任何步骤失败
-- 时整体回滚，幂等记录与 transition/周期/覆盖变更同事务；同 key 重试
-- 重放历史结果，绝不重复执行转让。
-- (human_id, transition_id) 以 DEFERRABLE 复合外键对应
-- group_owner_transitions 的同一条真实事实行（parent key：
-- UNIQUE (old_owner_participant_id, transition_id)），精确证明请求者
-- human_id 正是该次转让的旧 owner；Room、目标与新 owner 周期事实只从
-- 不可变 transition 读取（同 key 重放 JOIN transition），不在幂等表重复
-- 存储。FK 不绑定 active_marker：新 Owner 再次转让后旧 key 仍可合法历史
-- 重放（transition 行永存且不可变）。
CREATE TABLE group_owner_transfer_idempotency (
    human_id      TEXT NOT NULL REFERENCES participants(id),
    key           TEXT NOT NULL,
    transition_id TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (human_id, key),
    FOREIGN KEY (human_id, transition_id)
        REFERENCES group_owner_transitions(old_owner_participant_id, transition_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (length(key) BETWEEN 1 AND 128),
    CHECK (key = trim(key)),
    CHECK (instr(key, char(0)) = 0),
    CHECK (instr(key, ',') = 0),
    CHECK (key NOT GLOB '*[^!-~]*')
);

-- 幂等记录整行不可改写。
CREATE TRIGGER group_owner_transfer_idempotency_immutable
BEFORE UPDATE OF human_id, key, transition_id, created_at
ON group_owner_transfer_idempotency
FOR EACH ROW
WHEN NEW.human_id IS NOT OLD.human_id
  OR NEW.key IS NOT OLD.key
  OR NEW.transition_id IS NOT OLD.transition_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'group owner transfer idempotency records are immutable');
END;

CREATE TRIGGER group_owner_transfer_idempotency_no_delete
BEFORE DELETE ON group_owner_transfer_idempotency
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'group owner transfer idempotency records cannot be deleted');
END;

-- 身份防改写：REPLACE 以主键 (human_id, key) 冲突时先隐式删除冲突行；
-- 本守卫在 INSERT 阶段直接拒绝 (human_id, key) 已存在的写入。
CREATE TRIGGER group_owner_transfer_idempotency_insert_identity_guard
BEFORE INSERT ON group_owner_transfer_idempotency
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM group_owner_transfer_idempotency
    WHERE human_id = NEW.human_id AND key = NEW.key
)
BEGIN
    SELECT RAISE(ABORT, 'group owner transfer idempotency identity already exists');
END;

-- Agent revoke 传播：Agent 变为 revoked 的同一事务内，用 NEW.revoked_at
-- 将该 Agent 的全部 active Membership 周期关闭（closed_by=agent_revoked，
-- left_before_sequence 固化该 Room 当前最大 message sequence——每个 Room
-- 独立确定，同一事务内一致）；Room 与 Human Membership 保持 active；
-- 重复撤销（已 revoked）不触发本触发器。messages 表由 005 迁移创建，
-- 触发器在运行时 DML 才解析表引用，因此这里可以引用尚不存在的表。
CREATE TRIGGER agent_revoke_propagates_room_memberships
AFTER UPDATE OF status ON participants
FOR EACH ROW
WHEN OLD.kind = 'agent' AND OLD.status = 'active' AND NEW.status = 'revoked'
BEGIN
    UPDATE room_memberships
       SET active_marker = 0,
           left_at = NEW.revoked_at,
           left_before_sequence =
               (SELECT COALESCE(MAX(sequence), 0) FROM messages
                WHERE room_id = room_memberships.room_id),
           closed_by = 'agent_revoked'
     WHERE participant_id = NEW.id AND active_marker = 1;
END;
