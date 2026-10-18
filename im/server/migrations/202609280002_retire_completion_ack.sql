-- 202609280002_retire_completion_ack.sql
--
-- Completion / ACK（旧投递模型）整体退役——**代码面**（持久面同刀）。
--
-- 授权：主人 2026-09-28 裁决「不符合 IM 的就去了」+「去掉了吧」。
-- 前置：2026-09-24 主人已裁「Completion 与 ACK/投递位置机制退役」（零生产流量、
--       零真实消费者；当刀只做文档层，代码与表一字未动）；2026-09-28 Hermes
--       connector 随「不做第三方 Agent 适配器」删除后，**唯一消费者消失**。
-- 原理一句话：**投递确认不是 IM 事实**——Completion/ACK 把"执行端收到了没有 /
--       处理完了没有"变成公共世界里的耐久事实；拉取模型下 Event 是 Core 单方面
--       建立的可恢复投递记录，Backend 自己拉、自己说（say）、自己收口（Runtime
--       私有终态），**没有任何东西需要向 im 侧确认**。
--
-- 本迁移只做两件事：
--   1. fail-closed 断言：两张表都必须为空（本刀**不搬行**——有行就停下来报告，
--      绝不静默丢事实）；
--   2. DROP 两张表（表上的守卫与索引随表消失；实测全库没有任何**挂在别的表上**
--      的触发器/视图引用它们）。
--
-- 既有 migration 一律不改（sqlx `_sqlx_migrations` 带 checksum，改一行就会让
-- 真家启动失败）：`202608110008_completions.sql`、`202608140002_agent_delivery_
-- positions.sql` 与 `202608130001_message_recall.sql` 里的相关定义**按原样留在
-- Git 历史里**——它们是"当时发生过什么"的记录，不是活契约。

-- ── 1. fail-closed：两表必须为空 ─────────────────────────────────────────
CREATE TEMP TABLE retire_completion_ack_guard (
    x INTEGER NOT NULL CHECK (x = 0)
);

INSERT INTO retire_completion_ack_guard (x)
SELECT (SELECT COUNT(*) FROM completions) + (SELECT COUNT(*) FROM agent_delivery_positions);

DROP TABLE retire_completion_ack_guard;

-- ── 2. 两张表退场（各自 4 / 7 道守卫随表消失）──────────────────────────
DROP TABLE completions;
DROP TABLE agent_delivery_positions;
