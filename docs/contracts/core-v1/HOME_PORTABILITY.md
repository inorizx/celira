# HOME_PORTABILITY.md — Home Portability（第一层：离线备份 / 校验 / 保真恢复）

> 状态：**第一层（L0/L1）已实现并经独立 Core 进程黑盒验收**。本层只做**保真 Disaster Restore**：把一个 Home 的持久公共事实（`core.db` 与 `<media_root>/attachments/` 字节命名空间）离线备份、校验并按原样恢复，Core v1 公共协议、HTTP API、SQLite migration、Runtime 与 Local Client Host 行为**一律不改**。
>
> 权威工具实现：`tools/home_portability.py`（仅 Python 3 标准库）。权威契约以本文为准；文档、实现与测试冲突必须按根 `../../../AGENTS.md` 先报告。
>
> **Secure Relocation / 加密 Export / Import / `--home-id` 改写 / 在线备份 / 多副本 / fencing 不在本层**，另立切片，绝不外推（见 §7）。

## 1. 一句话原理

Home Portability 第一层把「Home 的休闲性重启/迁移」窄化为一次**明确的离线停机操作**：操作者确认旧 Core 已停止（`--confirm-core-stopped`，不提供任何双 Active Core 检测/fencing）后，工具把 SQLite 公共事实与附件字节命名空间原样搬到「备份目录」，restore 时先完整校验、再在 sibling 临时目录重建并原子落到新空目录；`core.db` 原样保留 token_hash 等摘要，使**旧 Human Session / Pairing Grant 状态 / Backend Binding 与 Credential 摘要语义随行**，恢复后持有原 token 的设备与 Runtime 无需重新配对。

## 2. C4 Level 1–2 拓扑（本层新增容器与连接）

```text
L1：Home（权威单元）= 呈现平面(Client) + 公共事实平面(恰一个 Active Core，单写者)
     + 执行平面(Agent+Runtime)。任一时刻一个 Home 一个可写逻辑 Core。

L2：新增一个离线运维容器（不属于产品角色、不是 Participant、不是事实源、不是写者）
┌─ Home Portability Tool（tools/home_portability.py，一次性脚本进程）────────┐
│  backup : <db 目录>/core.db ──只读──> 备份目录（manifest.json + core.db     │
│           <media_root>/attachments ──只读──> 备份目录/media/attachments/*   │
│  verify : 备份目录 ──只读校验（绝不修改）──> 非秘密摘要                       │
│  restore: 备份目录 ──校验──> sibling 临时目录 ──原子 rename──> 目标目录        │
│           只能产出 core.db + media/attachments/* + restore-manifest.json     │
└─────────────────────────────────────────────────────────────────────────────┘
  active 边界：工具绝不连接 Core HTTP、绝不启动 Core、绝不读 Runtime / Local
  Client Host / Provider / operator 凭据、绝不复制任何 raw token。
  恢复目标随后由 Active Core（同一 celira-core 二进制）启动消费：
  Active Core ──公开 HTTP（/health /ready /v1/home /v1/backend/events…）──> 黑盒核验
```

- **不新增第二个公共事实写者**：工具不写 Core 数据库语义，只做文件系统级搬运；恢复后的数据源仍只有一个可写逻辑 Core（单写者不变量由部署/运维承担）。
- **不新增依赖**：纯 Python 3 标准库（`sqlite3`、`hashlib`、`json`、`os`、`shutil`、`argparse`）。
- **离线边界**：备份/恢复全程不依赖 Core 进程在线；恢复只提供「数据文件」，由操作者决定何时启动。

## 3. 事实边界（本层搬运什么、不碰什么）

搬运（原样、逐字节）：

| 事实 | 备注 |
|---|---|
| `core.db`（SQLite 公共事实） | 内含 Home Identity、Human Session 摘要、Pairing Grant、Backend Binding/Credential 摘要、Room/Membership/Message/Event/Completion/ACK/附件元数据/撤回/read state 等全部冻结公共事实；**原样保留 token_hash，旧 token 语义不变** |
| `<media_root>/attachments/` 全部条目 | 只认 Core 受控命名空间：严格 UUID v4 文件，与 Core 自有临时名 `<uuid>.tmp` / `<uuid>.tmp.<uuid>`；任一其它名字（子目录、未知名、符号链接、非普通文件）→ **backup 明确失败**，绝不静默丢弃 |

绝不触碰 / 绝不复制：

- Runtime 私有 SQLite、Runtime master key、Local Client Host Secret Store / browser 会话 / provisioning.json；
- Provider/backend/operator/Human/pairing 的**原始 token**（工具不打印、不落盘、不提取）；
- Client / Runtime / Host 的进程状态、运行目录、配置；
- restoration 也不启动 Core、不自动改既有配置。

媒体根目录结构约束（与 Core 一致，`src/composition/mod.rs`）：附件字节位于 `<media_root>/attachments/` 单层受控目录；Core 不会在其中创建子目录。工具校验 `<media_root>/attachments` 必须是目录。

## 4. manifest.json（版本 1，非秘密）

备份目录布局（目录格式，不依赖外部 tar）：

```text
<output>/
  manifest.json          （只读权威索引，最后写入）
  core.db                （字节原样拷贝）
  media/attachments/<strict-uuid>            （每个附件）
  media/attachments/<uuid>.tmp[.<uuid>]      （Core 自有临时残留，同样搬运）
```

`manifest.json` 字段（全部非秘密）：

```jsonc
{
  "format_version": 1,
  "tool": "tools/home_portability.py",
  "home_id": "<uuid v4>",                 // 来自 home 表，非秘密
  "home_display_name": "… 或 null",       // 可选，非秘密
  "created_at": "<ISO8601 UTC>",
  "applied_migrations": [                 // 仅 version/description/success，绝不含 checksum 原文
    { "version": 202608110001, "description": "core_baseline", "success": true }
  ],
  "core.db": { "size": 12345, "sha256": "<hex>" },
  "attachment_count": 3,
  "attachment_bytes": 1234,
  "attachments": [
    { "relative_path": "media/attachments/<uuid>", "size": 10, "sha256": "<hex>" }
  ]
}
```

- `relative_path` 只允许 `media/attachments/<单段名>`，名字必须是受控命名空间；
- 附件条目按文件名字典序，确定性输出；
- manifest 中所有 `sha256` 都是对**文件内容**（`core.db` / 附件字节流）计算的内容摘要；`attachment_count` / `attachment_bytes` 必须是**非负整数**，并分别与 `attachments` 数组长度和条目 `size` 之和**严格相等**（verify 会强校验，见 §5.2）；
- manifest **绝不包含**：raw token、token_hash、credential hash、SQLx migration 的 checksum 原文（`applied_migrations` 只有 version/description/success），也不含正文或绝对路径。

## 5. 工具契约

统一用法：

```bash
python3 tools/home_portability.py backup  --db PATH --media-root PATH --output PATH --confirm-core-stopped
python3 tools/home_portability.py verify  --backup PATH
python3 tools/home_portability.py restore --backup PATH --destination PATH --confirm-core-stopped
python3 tools/home_portability.py inspect --backup PATH
```

退出码：`0` 成功；`1` 操作失败（校验/完整性/文件系统/权限/失败路径）；`2` 用法错误（例如缺少 `--confirm-core-stopped`）。成功向 stdout 输出一行非秘密 JSON 摘要（home_id、migration 数/最高版本、附件数/总字节），并回显操作者传入的 `output`/`destination`/`backup` 路径（原样回显、可能是绝对路径，属非秘密）；失败向 stderr 输出稳定分类，绝不携带 token/hash/正文/秘密哈希。

### 5.1 backup

- 只接受显式 `--confirm-core-stopped`；`--db` 必须是**普通文件**（非符号链接、非目录），`--media-root` 与 `--media-root/attachments` 必须是**目录**；
- 若 `core.db` 旁存在 `-wal` / `-shm` / **`-journal`**（未 checkpoint / 未回滚的 SQLite 日志）→ **失败** `db_unclean_stop`（要求干净停止，绝不承诺恢复未落盘数据）。
  ⚠ 2026-09-25 修正：原先只查 `-wal` / `-shm`，而**本构建用的是 SQLite 默认的 rollback journal**（`im/server/src` 里没有任何 `journal_mode` pragma）⇒ 硬停留下的是 `core.db-journal`，那道 fail-closed **永不触发**。三种后缀现在都查（真机 `im/server/data/*/` 里就躺着 `core.db-journal`）；
- 先以只读打开 DB 跑 `PRAGMA integrity_check` 与 `PRAGMA foreign_key_check`（任何异常 → 失败，避免把坏库做成备份），并读取 `home`（恰一行、规范 UUID v4）与 `_sqlx_migrations`（全部 `success=1`，否则失败）；
- `--output` 必须**不存在**（绝不覆盖）；输出不得位于媒体根或 DB 目录之内（防自嵌套）；输出目录 `0700`、文件 `0600`（显式设置，不依赖 umask）；
- 写入原子：同目录唯一临时名（O_EXCL）+ fsync + rename + 父目录 fsync（best-effort）；**manifest 最后写入**，任何失败清理半成品并退出非零；
- core.db 与每个附件在复制前预读 size+sha256，复制后逐项比对流式拷贝返回的 size/digest；不一致（源文件在备份途中变化）抛 `database_changed_during_backup` / `attachment_changed_during_backup` 并清理 output；
- 输出非秘密摘要，不打印任何 hash/token。

### 5.2 verify

- 只读，绝不修改备份（SQLite 以 `mode=ro` + `query_only` 打开）；
- **显式结构检查**（不使用 `os.walk`，杜绝 symlink 目录被静默跳过）：备份根**恰好** `manifest.json`/`core.db` 两个普通文件 + `media` 一个非 symlink 目录（无其它文件/目录）；`media/` 恰好含 `attachments` 一个非 symlink 目录（无额外条目）；`attachments` 必须存在（即使 manifest `attachment_count=0`），且只含 manifest 列出的非 symlink 普通文件——子目录、额外文件、symlink、缺目录一律非零；
- 校验：manifest schema/`format_version`（拒绝未知字段、未知版本）、`tool` 必须等于 `tools/home_portability.py`、`created_at` 必须是字符串、home_id 合法性、migration 全部 success；**路径安全**（拒绝绝对路径、`..`、NUL、越出 `media/attachments/<单段名>`）；`attachment_count`/`attachment_bytes` 必须是非负整数，并严格等于 `attachments` 数组长度与条目 `size` 之和；
- 校验 `core.db` 与每个附件的 size + sha256 与 manifest 一致，且逐项校验后的实际附件总字节必须等于 `attachment_bytes`；再对 `core.db` 跑 `PRAGMA integrity_check`、`PRAGMA foreign_key_check`、home 单例读取、`_sqlx_migrations` 全部 success，且与 manifest 的 home_id/migrations 一致；
- 报告：home_id、migration 数/最高版本、附件数/总字节、结果 OK；不报告 raw token、token_hash、credential hash、migration checksum 原文、正文；
- 对不完整/篡改/结构越界/附件缺失/路径逃逸一律退出非零。

### 5.3 restore

- 顺序 = **先完整 verify**（任一失败即退）→ 目标必须**不存在**或是**空目录**（非空/文件/symlink → 失败）→ 在**目标同父目录的 sibling 临时目录**（`0700`）完整重建 `core.db` + `media/attachments/*` 并逐个复验与 manifest 一致 → 写入非秘密 `restore-manifest.json`（绝不复制 token）→ **原子 `os.rename` 到目标** → 复核目录 `0700`/文件 `0600`；
- 恢复内容只包括 `core.db`、`media/attachments` 与恢复结果 marker；不启动 Core、不修改既有配置、不复制 Client/Runtime 状态；
- 拒绝符号链接/路径穿越（来源已在 verify 排除）。

### 5.4 inspect

只读 manifest，输出非秘密摘要（供脚本/黑盒读取 home_id、migration 版本、附件数），不触碰 DB。

## 6. 重要语义与安全边界（如实声明）

- **`--confirm-core-stopped` 只是操作者确认**：工具不能、也不声称检测或阻止双 Active Core；**Home Identity 不是 fencing**。单 Active Core 仍是部署/运维单写者不变量，不由本工具保证（同 `HOME_ACCESS.md` §5 第 9 条 / `../../../ARCHITECTURE.md` §1）。
- **保真恢复语义**：`core.db` 原样包含 token_hash，故持有原 Human Session / Backend Credential / Pairing Grant token 的设备或 Runtime 在恢复后可继续使用；Pairing Grant 的 active/consumed/revoked/expired 语义保持不变（未消费 Grant 恢复后仍可兑换）。
- **附件维护语义保留**：pending attachment 是由 Core 启动维护清理的 24 小时私有暂存租约；若停机时间超过 24 小时，恢复后 Core 启动会按既有契约回收过期 pending 并清扫孤儿文件——工具**不伪造保证**，「超过 24h 停机可能丢失未绑定的 pending 附件」应在备份与文档层面如实呈现。绑定的（bound）附件是公共事实，永不因该维护被删。
- **未知字节不被静默打包**：attachments 命名空间之外的文件、非 Core 命名空间的未知名、符号链接、子目录一律 fail closed（`attachment_unknown_name` / `attachment_symlink` / `attachment_not_regular`），不打包、不静默丢弃。
- **秘密卫生**：manifest 及工具输出只含文件内容的 sha256 内容摘要；**绝不包含** raw token、token_hash、credential hash、SQLx migration 的 checksum 原文或正文。成功输出会回显操作者传入的 `output`/`destination`/`backup` 路径（原样回显、可能是绝对路径，非秘密）；工具**不承诺**「永不输出绝对路径」。
- **磁盘权限是最小化纵深防御，不是 OS 沙箱**：不能防御 root、同 UID 恶意进程或已攻陷操作系统（与 `../../client/LOCAL_CLIENT_HOST.md` §7 威胁边界一致）。

## 7. 明确不做（本层边界）

- Export / Import（含加密导出、跨 Home 迁移、匿名/可转移身份）；
- Secure Relocation（密钥轮换、重新加密、身份接管）；
- 在线备份、WAL/checkpoint、PostgreSQL 或其它存储后端；
- 双启动检测、fencing、HA、多副本写、备用写者；
- 新 Core API、改 Core 公共协议/HTTP/SQLite migration/Runtime/Client Host 行为；
- 把 `media_root` 全目录的未知文件静默打包为附件；
- 从备份提取/展示/转移任何原始 token。

## 8. 验证与验收

### 8.1 工具自身测试（stdlib unittest，无第三方依赖）

```bash
python3 -m unittest discover -s tools/tests -p 'test_*.py' -v
```

覆盖：backup→verify→restore 成功 roundtrip（含真实 SQLite、`<uuid>.tmp` 残留、权限 0700/0600、manifest 无 checksum/无 token）；篡改 manifest/DB/附件、附件缺失、多余文件、路径穿越（`../`、绝对路径、NUL）、未知附件名、attachments 中子目录、符号链接、`-wal`/`-shm` sidecar、缺 `home` 表、migration `success=0`、目标非空/为文件、缺 `--confirm-core-stopped`、verify 不改备份、恢复失败不留临时 sibling。

### 8.2 真实进程黑盒（独立 celira-core 二进制 + 公开协议）

```bash
CARGO_INCREMENTAL=0 cargo build --bin celira-core
python3 tools/tests/blackbox_home_portability.py [路径/celira-core]
```

黑盒步骤（全部使用公开协议，不直读数据库作为成功证据）：

1. 真实 Core 启动 → `bootstrap-human` 取 Human Session（token 仅进程内存）；`GET /v1/home` 取 home_id；
2. 公开 API：`POST /v1/agents`、`POST /v1/direct-rooms`、`POST /v1/agents/{id}/backend-bindings`、`POST .../credentials`（Backend Credential）、`POST /v1/rooms/{id}/attachments`（multipart 上传）→ 绑定 Message → 下载附件复验原始字节；`issue-human-session` 与 `issue-pairing-grant` 各取一个预恢复 token；
3. 干净停止（确认无 `-wal`/`-shm`）→ `backup` → `verify` → `restore`；
4. 用恢复后的 `core.db` + `media` 启动新 Core：`GET /health`、`GET /ready` 匿名 200；**旧 Human Session** `GET /v1/home` → 200 且 home_id 不变；第二 Session 可用；**Pairing Grant 恢复后可兑换**且新 Session 可用；**Backend Credential** 仍可认证 `GET /v1/backend/events`；附件用旧 Session 下载字节一致；整个 attachments 命名空间完整；
5. 失败路径抽检：缺 `--confirm-core-stopped` → 退出 2；篡改附件 → verify 非零。

当前真实结果：29/29 步骤 PASS，恢复后 `home_id` 不变、旧 Session/Grant/Backend Credential 全部可用、附件字节逐字节一致。

## 9. 残余风险与被阻塞项

- **双启动**：本层不检测、不保证「同一备份不被同时启动两次」；由运维单写者不变量承担。
- **pending 附件**：超过 24h 停机窗口的未绑定 pending 可能被 Core 启动维护回收，属既有契约，不伪造保证。
- **介质安全**：备份目录本身未加密、未混淆；保护备份介质（权限/磁盘加密/冷备箱）是操作者职责。
- **未 checkpoint 的 WAL**：若 Core 非干净停止留下 `-wal`/`-shm`，backup fail closed（要求干净停止）。
- Secure Relocation / Export / Import / 在线迁移为后续切片，本层不抢占。
