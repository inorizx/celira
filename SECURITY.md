# Security policy

English | [简体中文](#中文)

## Reporting a vulnerability

**Use GitHub's private vulnerability reporting**: the **Security** tab of this repository →
**Report a vulnerability**. That channel is private between you and the maintainers.

Please **do not** open a public issue for a vulnerability, and **do not** paste live credentials
into an issue, a pull request or a chat. In a report, useful things are: what you ran, what you
expected, what happened, and the smallest reproduction you have. If a credential is involved, say
**which kind** it is (see "Credentials" below) and **rotate it yourself first** — never send the
value.

There is no bug-bounty programme and no dedicated security team: this is a small project, and we
answer as fast as we can.

## In scope

- **the world**: `im/server/` — the public API, the migrations, the im server itself;
- **the execution end**: `agent/` — the runtime, the OS surface, the apps, the storage layer;
- **the client and the desktop shell**: `client/`, `app/`;
- **the home tooling**: `home.js` and `scripts/`.

## Not in scope

- **your own home's configuration and secrets** — a `home.conf` you wrote, a key you pasted, a world
  you exposed to the internet without TLS;
- a terminal declared with **whole-machine trust** (`--terminal full`) doing exactly what it says it
  does;
- an Agent doing something you dislike with the capabilities you gave it. That is a product
  judgement, not a vulnerability — the system does not think for the Agent (see the README's
  "What it is not").

## Credentials — the discipline this project holds itself to

Three kinds of credential exist, and **none of them is meant to travel**:

- **the model key** — stays on your machine, goes to your model provider; it never enters the
  world's database and never reaches a browser;
- **the home key** — a **per-machine possession proof** for the local doors, not an identity; sent
  only in an `Authorization: Bearer` header and stored only by the client that holds it;
- **the world's backend credential** — belongs to a binding; it can be rotated and revoked.

So **a credential in a URL, in a log, or in a rendered page is a defect** — report it as one. The
guards that enforce this, and what each of them cannot see, are listed in
[`AGENTS.md`](./AGENTS.md) §「验证与汇报」.

## 中文

**报漏洞**：用本仓 **Security** 页的 **Report a vulnerability**（GitHub 的私密漏洞上报通道）——
那是你与维护者之间的私密路径。**不要**为漏洞开公开 issue，**也不要**把真实凭据贴进 issue、PR
或聊天：说明**是哪一类**凭据（见上），并**自己先轮换掉**，别把值发出来。
没有漏洞赏金，也没有专职安全团队——这是个小项目，我们会尽快回应。

**在射程内**：`im/server/`（世界）· `agent/`（执行端）· `client/` 与 `app/` · `home.js` 与 `scripts/`。
**不在射程内**：你自己那份家的配置与钥匙 · 声明了整机全权（`--terminal full`）的终端照它说的做 ·
Agent 拿着你给的能力做了你不喜欢的事（那是产品判断，不是漏洞——系统不替 Agent 思考，见 README
的「What it is not」）。
**凭据纪律**：模型 key 只到那台机器为止、永不进世界的库、永不回浏览器；家钥匙是**按机器**的持有证明
（不是身份），只走 `Authorization: Bearer`、只存在持有它的客户端；世界的后端凭据属于绑定，可轮换、可撤销。
⇒ **凭据出现在 URL／日志／渲染页里，就是缺陷**，按缺陷报。执行这些判据的守卫、以及每条守卫
**看不见什么**，列在 [`AGENTS.md`](./AGENTS.md) 的「验证与汇报」。
