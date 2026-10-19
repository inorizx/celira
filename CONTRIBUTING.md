# Contributing

English | [简体中文](#中文)

Thanks for looking. Two different things live in this repository, and they have different doors:

- **adding something to Celira** (a skill pack, an MCP server, a theme) — you do **not** need to touch
  this repository at all. Start at [`EXTENDING.md`](./EXTENDING.md), which lists the four extension
  slots and what each one can and cannot do;
- **changing this repository** (code, docs, contracts) — that is what this file is about.

## Before you open a pull request

- **Open an issue first for anything non-trivial.** This project has an explicit authority order
  ([`AGENTS.md`](./AGENTS.md) §「权威文档顺序」) and some areas are **frozen** (the Core v1 contracts)
  or **ruled** (the product shape). A patch that changes a public contract, a persisted format, a
  recovery semantic or a security trade-off has to be ruled on before it is written, not after.
- **Keep the patch about one thing.** The repository's own discipline is "one concept, one name" and
  "a patch must not carry another workstream's changes"; wholesale renames or reformatting are
  rejected even when they are mechanically correct.
- **Match the house voice where it applies.** Text the *Agent* reads (capability descriptions,
  receipts, opening blocks) is written with identifiers and structured fields — no metaphors, no
  person pronouns. The naming authority is
  [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md): check it **before**
  coining a word.

## Gates you can run yourself

Everything below is self-contained: no world, no model, no credentials.

| What you touched | Run |
|---|---|
| anything | `node scripts/check-docs.mjs` · `node scripts/check-public-hygiene.mjs` |
| `agent/` | `cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` · `cargo test --all-targets` (all three with `--manifest-path agent/Cargo.toml`) |
| `im/server/` | the same three, from the repository root |
| `client/` | `cd client && npm run verify` |
| `app/` (desktop shell) | `cd app && npm run build -- --no-bundle` |

CI runs the self-contained subset of exactly these. **A green CI does not mean everything passed** —
the workflow file says which gates it deliberately leaves out and why.

## Gates only a human can run

Some gates need a real world, a real model key, and **spend real model calls**. They are not in CI
and they are not expected from a first-time contributor:

- `probe-loop.js` · `probe-recall.js` · `probe-zero.js` · `probe-service-lifetime.js` ·
  `probe-vision.js` · `probe-handoff.js` — each one's trigger condition, exit codes and **what it
  cannot see** are written down in [`AGENTS.md`](./AGENTS.md) §「验证与汇报」.

If a gate is **not applicable** on your machine (no model key, no browser), report it as `N/A` —
that is neither a pass nor a failure, and pretending otherwise is the one thing this project treats
as a defect in its own tooling.

## What will be declined

This project publishes its non-goals, so you can check before writing code: see the README's
**"What it is not"** and [`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md) §8,
plus the architecture red lines in [`AGENTS.md`](./AGENTS.md) §「架构红线」. A patch that pulls in one
of those directions is not a matter of taste — the reason is written at those two places.

## Licence

Celira is released under the **MIT licence** ([`LICENSE`](./LICENSE)). By opening a pull request you
agree that your contribution is distributed under the same licence.

## Security

Do **not** report a vulnerability in a public issue: [`SECURITY.md`](./SECURITY.md) has the private
channel, what is in scope, and the credential discipline this project holds itself to.

## 中文

**两件不同的事，两扇不同的门**：**给 Celira 加东西**（技能包 / MCP server / 主题）**不必动本仓**——
从 [`EXTENDING.md`](./EXTENDING.md) 开始（四格各自能做什么、不能做什么都在那里）；**改本仓**
（代码、文档、契约）才是本文要说的事。

**开 PR 之前**：非平凡的改动**先开 issue**——本仓有明文权威顺序（[`AGENTS.md`](./AGENTS.md)
的「权威文档顺序」），有些区域是**冻结的**（Core v1 契约）、有些是**已裁的**（产品形态）⇒ 动公共契约、
盘上格式、恢复语义或安全取舍的补丁要**先裁决再写**，不是写完再问。**一个补丁只做一件事**：
本仓的纪律是「一个概念一个名称」＋「不得夹带别的工作流的改动」，整片改名或重排即使机械上正确也会被拒。
**声音**：给 **Agent** 读的文本（能力说明、回执、开场块）用标识符与结构化字段——不用比喻、不用人称代词；
名字的权威是 [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md)，**造词之前先查它**。

**你自己能跑的门禁**（都不连世界、不调模型、不要凭据）：`node scripts/check-docs.mjs` ·
`node scripts/check-public-hygiene.mjs`；`agent/` 与 `im/server/` 各跑 `cargo fmt --check` ·
`cargo clippy --all-targets -- -D warnings` · `cargo test --all-targets`；`client/` 跑
`cd client && npm run verify`；壳跑 `cd app && npm run build -- --no-bundle`。
CI 跑的就是这一批里**自洽的那个子集**——**CI 绿不等于全过**，哪些故意不在 CI 里、为什么，
写在 workflow 文件里。

**只有人能跑的门禁**：`probe-*` 那几条要真世界、真模型钥匙，**且花真钱** ⇒ 不在 CI 里，也不指望
第一次贡献的人跑；每条的触发条件、退出码与**它看不见什么**写在 [`AGENTS.md`](./AGENTS.md) 的
「验证与汇报」里。**不适用就报 `N/A`**——既不算通过也不算失败；假装通过才是本仓认定的缺陷。

**会被拒的方向**：本仓把非目标写出来了——README 的「What it is not」＋
[`PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md) §8 ＋ [`AGENTS.md`](./AGENTS.md) 的
「架构红线」。往那些方向走的补丁不是口味问题，理由写在那两处。

**许可**：MIT（[`LICENSE`](./LICENSE)）；提交补丁即表示你同意你的贡献以同一许可分发。
**安全**：漏洞**不要**开公开 issue——[`SECURITY.md`](./SECURITY.md) 有私密通道、射程与凭据纪律。
