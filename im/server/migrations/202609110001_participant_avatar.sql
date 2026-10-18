-- Celira Core V1：Participant Avatar P0（头像 participant 窄通道）。
--
-- 契约：CORE_V1_AVATAR_P0.md（唯一权威，2026-09-11 主人批准）
--
-- 形状：Profile 版本链 participant_profile_versions 追加可空 avatar 字段族
-- （内部 media 引用 avatar_storage_key + avatar_content_type +
-- avatar_size_bytes + avatar_sha256_hex + avatar_updated_at）。头像写入/摘除 =
-- 该链追加新版本（profile_version +1），与文本字段共用同一版本链、同一套
-- Idempotency-Key + base_version 裁决、同一份 participant_profile_put_idempotency
-- 幂等记录表（canonical 摘要形状不同即 409 冲突，无需改表）。
--
-- 追加式纪律：ALTER TABLE ADD COLUMN 只追加可空列，既有行（avatar 全 NULL）
-- 与既有表语义完全不变；既有触发器（immutable / no-delete / identity guard /
-- 严格单调 / published_by_human / target_owned_agent）不受影响。
--
-- avatar 字段族的全部或全无由 BEFORE INSERT 触发器裁决（ADD COLUMN 无法
-- 追加跨列 CHECK）；各列自身的形状由列级 CHECK 裁决：storage_key 严格
-- UUID v4（与 attachments 同一谓词）、content_type 白名单（image/png |
-- image/jpeg | image/webp——头像是内联展示图，排除 SVG/HTML 等可执行类型）、
-- size_bytes 1..=2 MiB、sha256_hex 64 位小写 hex、updated_at 正整数。
--
-- 幂等记录复用 participant_profile_put_idempotency：request_digest 是含
-- base_version 的 canonical JSON 摘要（文本 PUT / 头像 PUT / 摘除三族形状
-- 互不相同），同 key 异摘要 → idempotency_conflict；同 key 同摘要恒重放。
--
-- 本迁移不新增表、不新增事件类型、不产生实时推送；旧版本字节 GC 是运维
-- 事项，本切片不做（orphan sweep 只回收从未被任何版本行引用的崩溃残留）。

ALTER TABLE participant_profile_versions ADD COLUMN avatar_storage_key TEXT
    CHECK (avatar_storage_key IS NULL
        OR (typeof(avatar_storage_key) = 'text'
            AND length(avatar_storage_key) = 36
            AND substr(avatar_storage_key, 9, 1) = '-'
            AND substr(avatar_storage_key, 14, 1) = '-'
            AND substr(avatar_storage_key, 19, 1) = '-'
            AND substr(avatar_storage_key, 24, 1) = '-'
            AND substr(avatar_storage_key, 15, 1) = '4'
            AND substr(avatar_storage_key, 20, 1) IN ('8', '9', 'a', 'b')
            AND avatar_storage_key = lower(avatar_storage_key)
            AND instr(avatar_storage_key, char(0)) = 0
            AND avatar_storage_key NOT GLOB '*[^0-9a-f-]*'
            AND length(replace(avatar_storage_key, '-', '')) = 32));

ALTER TABLE participant_profile_versions ADD COLUMN avatar_content_type TEXT
    CHECK (avatar_content_type IS NULL
        OR avatar_content_type IN ('image/png', 'image/jpeg', 'image/webp'));

ALTER TABLE participant_profile_versions ADD COLUMN avatar_size_bytes INTEGER
    CHECK (avatar_size_bytes IS NULL
        OR (typeof(avatar_size_bytes) = 'integer'
            AND avatar_size_bytes BETWEEN 1 AND 2097152));

ALTER TABLE participant_profile_versions ADD COLUMN avatar_sha256_hex TEXT
    CHECK (avatar_sha256_hex IS NULL
        OR (length(avatar_sha256_hex) = 64
            AND avatar_sha256_hex = lower(avatar_sha256_hex)
            AND avatar_sha256_hex NOT GLOB '*[^0-9a-f]*'));

ALTER TABLE participant_profile_versions ADD COLUMN avatar_updated_at INTEGER
    CHECK (avatar_updated_at IS NULL
        OR (typeof(avatar_updated_at) = 'integer' AND avatar_updated_at > 0));

-- 全部或全无：avatar 字段族要么整体缺失（文本 Profile 版本行），要么整体
-- 存在（头像版本行）。ADD COLUMN 无法追加跨列 CHECK，由 BEFORE INSERT 守卫
-- 裁决（版本行不可 UPDATE，因此只需防 INSERT）。
CREATE TRIGGER participant_profile_versions_avatar_all_or_nothing
BEFORE INSERT ON participant_profile_versions
FOR EACH ROW
WHEN (NEW.avatar_storage_key IS NULL) != (NEW.avatar_content_type IS NULL)
  OR (NEW.avatar_storage_key IS NULL) != (NEW.avatar_size_bytes IS NULL)
  OR (NEW.avatar_storage_key IS NULL) != (NEW.avatar_sha256_hex IS NULL)
  OR (NEW.avatar_storage_key IS NULL) != (NEW.avatar_updated_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'avatar fields must be all present or all absent');
END;
