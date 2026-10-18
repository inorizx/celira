-- 2026-09-28 · 邀请一个人进这个世界（第二半）：会话来源守卫认"这张邀请码点名的那个人"
--
-- 背景：`session_metadata_source_grant_same_human`（202608190001）要求
-- "来源 Grant 必须属于该 Session 的 Human"——那是**设备配对**的不变量
-- （新设备兑换为签发者本人的会话）。邀请类 Grant 的会话归**新来的那个人**，
-- 于是必须给守卫一个**可证明**的新分支，而不是把它放宽成"随便谁都行"。
--
-- 做法：Grant 行上记下"这张邀请码点名了谁"（`invited_human_id`），守卫只多认
-- 这一支：
--   session.human == grant.issued_by          （设备配对，既有语义一字不动）
--   OR session.human == grant.invited_human_id（邀请：**且只认这张码点名的那个**）
--
-- 为什么不是"grants_human = 1 就放行"：那样任何一张邀请码都能被用来给**任意
-- 既有 Human** 铸会话（守卫就废了）。点名 + 一次性写入让这条不变量仍然可证明：
-- 一张邀请码最多只能给一个人铸会话，且必须是它在同一事务里新建的那个人。
--
-- 只加一列 + 两道触发器重建：**不搬行、不改任何既有行**。

ALTER TABLE pairing_grants ADD COLUMN invited_human_id TEXT REFERENCES participants(id);

-- 点名的对象一经写入不可改写（`NULL → 有人` 是一次性事实；不允许换人、
-- 不允许抹掉）。它是**消费侧**事实，所以不进"签发事实不可改写"那张清单。
CREATE TRIGGER pairing_grants_invited_human_immutable
BEFORE UPDATE OF invited_human_id ON pairing_grants
FOR EACH ROW
WHEN OLD.invited_human_id IS NOT NULL
     AND NEW.invited_human_id IS NOT OLD.invited_human_id
BEGIN
    SELECT RAISE(ABORT, 'pairing grant invited human is immutable once set');
END;

-- 守卫重建：多认"这张邀请码点名的那个新 Human"这一支，其余逐字保留。
DROP TRIGGER session_metadata_source_grant_same_human;

CREATE TRIGGER session_metadata_source_grant_same_human
BEFORE INSERT ON session_metadata
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM human_sessions AS s
    JOIN pairing_grants AS g ON g.grant_id = NEW.source_grant_id
    WHERE s.id = NEW.session_id
      AND (s.human_id = g.issued_by OR s.human_id = g.invited_human_id)
)
BEGIN
    SELECT RAISE(ABORT, 'session metadata source grant must belong to session human');
END;
