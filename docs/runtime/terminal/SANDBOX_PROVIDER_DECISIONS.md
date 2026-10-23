# Celira Runtime 沙箱分层与可插拔后端裁决

> 状态：方向稿（2026-10-23）。先定契约，再落实现；本文不声称沙箱已经存在。
>
> 适用：`terminal` / `files` / `network` 这类会碰宿主的能力。`full` 仍是逃生门，不是默认档。

## 1. 一句话裁决

**权限按 Agent 长期给，沙箱按 Run 每次做；政策稳定，执行后端可插拔。**

- **Grant**：一支住户长期拥有什么能力、什么范围、多久、谁给的。
- **Sandbox**：每一段活动按 Grant 生成具体隔离；Grant 长期不等于执行时没有边界。
- **Approval**：高危动作即使有 Grant，也可以按次要求人确认；`full` 是 break-glass。

## 2. 为什么不照搬主流的“一次性沙箱”

主流 Agent 沙箱多按任务/会话给：任务开始建容器，任务结束扔掉。Celira 的住户有身份、记忆、关系和长期工作区，权限必须能跨 Run 存在。

所以：

- **Grant 的生命周期按 Agent**，不按单次任务；
- **隔离的执行单位仍是 Run/进程**，不因为 Grant 长期就退化成“整机裸跑”；
- 住户可以长期有 `terminal=sandbox`，但每一段活动仍要重新套沙箱。

## 3. 三层，不许混

```text
Grant（长期，按 Agent）
  terminal = none | sandbox | full
  files    = own | workspace | selected | full
  network  = none | allowlist | open
  mcp      = host-only | own-with-terminal
        │
        ▼
Sandbox（每次 Run 执行）
  workspace 可写、系统只读、默认断网、洗环境、限资源
        │
        ▼
Approval（高危动作，按次）
  删除大量文件 / 新域名 / 改系统配置 / full 的首次或超期
```

`Grant` 是政策，`Sandbox` 是执行，`Approval` 是刹车。三者不能互相替代。

## 4. 可插拔插在哪

**插在沙箱后端，不插在权限语义。**

固定不动的：

- `terminal` 只有 `none | sandbox | full` 三档；
- `sandbox` 的最低保证集固定；
- 谁能改 Grant、什么算高危、审计记什么，固定；
- Agent 只能请求档位，不能自己挑一个更弱的后端。

可插拔的：

- 具体用哪个隔离机制：bwrap、systemd-run、rootless 容器、微虚拟机、Windows AppContainer、macOS sandbox-exec；
- 后端声明自己**真的**能保证什么；
- 解析器按“所需保证 + 可用性”选择；没有合格后端就**拒绝**，不许静默退回宿主。

### 4.1 最小端口

```text
SandboxRequest
  workspace       工作区根
  read_only       只读挂载
  writable        额外可写路径
  network         none | allowlist
  env             允许进入子进程的环境变量
  limits          CPU / 内存 / 进程数 / 时间
  command         要执行的命令

SandboxProvider
  name            稳定名字（审计用）
  guarantees()    文件 / 网络 / PID / 环境 / 资源 / 持久工作区
  probe()         这台机器上可用吗；不可用给原因
  run(request)    执行并回传结果

SandboxResolver
  choose(required_guarantees) -> provider | SandboxUnavailable
```

`guarantees()` 不是广告词：后端声称有网络隔离，就必须有对应测试；没有这项保证的后端，不能被选来满足 `sandbox` 档。

## 5. Linux 默认链（不是唯一实现）

```text
bwrap
  ↓ 不可用（例如非特权 user namespace 被 AppArmor 限制）
SandboxUnavailable（明确拒绝；不退回宿主）
```

**`systemd-run --user` 不是兜底**：本机实测 systemd 255 在用户管理器下会接受 `ProtectSystem` / `ProtectHome` / `PrivateTmp` / `PrivateNetwork` 属性，但因为没有 CAP_SYS_ADMIN，挂载/网络命名空间根本没建，`ProtectSystem=strict` 下仍能写工作区外。它不是沙箱，不能拿来当 provider；只有固定参数的**系统级 helper**（需要单独的安全裁决）或容器/VM 才能作为后续 provider。

这条链只是**内置 provider 的默认顺序**。以后可以加 rootless 容器或微虚拟机，但不改变上面的 Grant 语义。

性能口径（本机实测/量级，不是承诺）：

- 裸跑 `/bin/true`：约 1–2 ms/次；
- bwrap：正常 Linux 上通常个位数到十几毫秒；本次验收机在 AppArmor 放开非特权 userns 后 **200 次约 908 ms（≈4.5 ms/次）**；限制恢复后按预期失败；
- `systemd-run --user`：约 20 ms/次，但**不是沙箱**，不列入 provider；
- 常驻 rootless 容器：冷启 100–500 ms，热 exec 10–50 ms，内存约 20–100 MB/Agent；
- 微虚拟机：启动 100–300 ms，内存约 30–128 MB/Agent。

对“偶尔跑命令、主要等模型”的住户，20 ms 级启动不是主要成本；**内存和“不可用时怎么办”才是**。

## 6. `full` 的边界

`full` 不是“更大的 sandbox”，是**没有 sandbox**：

- 明确显示“能改这台电脑”；
- 首次开启、重启后、或超过约定期限，重新确认；
- 建议只在专用机器/虚拟机开；
- 撤销必须在宿主停机时改配置，运行中改会被正在运行的住户改回去；
- 审计记录：谁在什么时候开了 full、哪一段活动用过。

不把“用户点了同意”当成安全机制；它只解决知情，不解决破坏。

## 7. 现在不做什么

- **不做公开插件市场**：先有 2–3 个内置 provider 和稳定端口，再谈第三方；否则供应链、版本和“自称沙箱”的问题会把安全边界弄花。
- **不做“所有后端等价”的假设**：没有保证集就不算 sandbox。
- **不做静默降级**：`sandbox` 不可用就拒绝，不自动退到 `full` 或宿主裸跑。
- **不把 Grant 写成一个 `agent.conf` 字符串就完事**：至少要有授予人、范围、到期、最后使用和审计；否则撤销与追责说不清。

## 8. 验收切片（下一步）

1. Linux：`terminal=sandbox` 的 provider 探测 + bwrap/systemd-run 二选一 + 不可用拒绝；
2. 同一组命令在 sandbox 与 full 下的文件/网络/环境边界测试；
3. 审计里记录 `provider`、`guarantees`、`grant` 与 `run`；
4. 再谈 rootless 容器/Windows AppContainer，不在第一刀里铺开。

**一句话**：可插拔的是“用哪只手执行”，不可插拔的是“谁有资格伸手、伸到哪、什么时候必须停”。
