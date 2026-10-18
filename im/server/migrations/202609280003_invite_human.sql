-- 2026-09-28 · 邀请一个人进这个世界（Pairing Grant 扩一类「签发给别人」）
--
-- 契约：`docs/contracts/core-v1/HOME_ACCESS.md`（Pairing Grant 语义表）
--       + `docs/contracts/core-v1/CORE_V1_ACCEPTANCE.md` 的解冻切片登记。
--
-- 一句话原理：**邀请是"由已有主体签发给一个新主体"的一次性凭据**——沿用既有
-- Grant 事实与状态机（一次性、可过期、可撤销、原子消耗、不可改写、不可删除），
-- 只把"兑换出来是谁的会话"从"签发者本人"扩成"也可以是新来的那个人"。
--
-- 只加一列 + 重建一道触发器：**不搬行、不改任何既有行**。
--   grants_human = 0：既有语义（新设备兑换为**签发者本人**的会话）——老客户端
--                     一字不改地继续工作；
--   grants_human = 1：兑换产出**一个新 Human**（display_name 由兑换者自己给）
--                     + 该 Human 的独立会话。
--
-- 既有判据一字不动：`issued_by` 必须是 Human、签发事实不可改写、consumed /
-- revoked 是终态（且 consumed 必须已有同 Grant 的 Session Metadata）、Grant
-- 无论何种状态都不可删除。

ALTER TABLE pairing_grants ADD COLUMN grants_human INTEGER NOT NULL DEFAULT 0
    CHECK (grants_human IN (0, 1));

-- 签发事实不可改写：把新列加进清单（其余判据逐字保留）。
DROP TRIGGER pairing_grants_issue_fields_immutable;

CREATE TRIGGER pairing_grants_issue_fields_immutable
BEFORE UPDATE OF grant_id, token_hash, issued_by, device_label, issued_at, expires_at, grants_human
ON pairing_grants
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'pairing grant issue fields are immutable');
END;
