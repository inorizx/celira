#!/usr/bin/env node
'use strict'
/**
 * Celira 家的入口 —— 一条命令把家点亮，不需要 Git Bash。
 *
 * ```text
 * node home.js up                 # 开机（家里缺什么补什么；第一次布置整个家）
 * node home.js up --agent <id>    # 给这一支 Agent 安家（世界里已有它 ⇒ 认领；家里可以有别的住户）
 * node home.js up --token         # 只补发一个 Human token（给 client 用）
 * node home.js status             # 看常驻件在不在 + 每个住户一支一行（身份/房/宿主/凭据/模型绑定）
 * node home.js doors              # 起**家的门**（常驻：只读状态口 ＋ 模型配置门，一个进程两个端面）
 * node home.js issue-home-key     # 发一把家钥匙（跨机照看；只打印一次）
 * node home.js down               # 关机（数据不动）—— 一个家可以住多支，一支一个环境根
 * node home.js down --agent <id>  # 只停这一支（世界、页面、别的住户一律不动）
 * ```
 *
 * 为什么不写 `.sh`：`bash` 在 Windows 上要借 Git for Windows 之身，而 `cmd` 里的 `bash`
 * 常常是 WSL 的启动器（撞上就报 Docker 磁盘找不到）——对一个"解压即用"的发布包来说，
 * 那是个不该存在的前置条件。Node 是发布形态**本来就有的**依赖（`serve_client.mjs` / vite），
 * 三平台原生，且是将来 npm 包 / Docker / app 里同一份货。
 *
 * 读数是中文、带 `[im]`/`[agent]`/`[client]`/`[token]` 前缀的稳定行（与 .sh 版同款），
 * 退出码语义与 `scripts/HOME_MANUAL.md` 那张表逐行对齐（见 `scripts/home/codes.js`）。
 *
 * 这一层**只做编排**：起停进程、读读数、把凭据经环境交给子进程。零第三方依赖，
 * 只用 Node 标准库。
 */

const { EXIT } = require('./scripts/home/codes')

const USAGE = [
  '用法: node home.js <up|status|doors|down|backup|config|issue-home-key|remote-control> [参数]',
  '  up     开机（缺什么补什么；第一次布置整个家）。一个家可以住**多支** Agent（住户）：',
  '         无参数 = 点亮家（世界/页面/家的门）+ **遍历每个住户各起一个宿主**（一支都没有时自建第一支）',
  '          可选： --token 补发 Human token ／ --agent <世界里的 Agent id> = **给这一支安家**',
  '          （世界里已有它 ⇒ 认领；家里已有别的住户不挡路 —— 它成为新的一支住户）',
  '          --attach-only = 只接上、不要求模型（没有模型来源时**接上但先不起它**）',
  '  status 看常驻件在不在 + **每个住户一支一行**（身份/房/宿主/凭据/模型绑定）',
  '  doors  起**家的门**并常驻：读端面（只读状态口，运行状态页的活源）＋ 写端面（模型配置门，',
  '         模型配置页的写口）—— **一个进程、两个端面**（两个端口、两套准入各判各的）',
  '  config 模型配置（没有 --serve 了：起门那件事现在归 doors 一条路）',
  '  issue-home-key 发一把**家钥匙**（跨机照看用；只打印一次，重跑=换新的一把）',
  '  remote-control on|off  **允不允许从别的机器接 Agent 到这台机器上**（缺省 off）。',
  '         家钥匙只回答"能不能进这台机器"；"允许我替你起进程"是另一件事，**只能在这台机器上开**',
  '  backup 把**整个家**复制一份（升级/回退前那一步；先验"没有一支在跑"，逐文件双侧 sha256）',
  '          可选： --to <目录>（不给就用 <家>-backup-<时间戳>）',
  '  down   关机（数据不动）。不带参数 = 停整个家；--agent <世界里的 Agent id> = **只停这一支**',
  '  （不带子命令 = up，`node home.js --token` 这类老写法也认）',
]

/**
 * 常驻件在命令行里**声明自己的家**（`--home <家目录>`）。
 *
 * 为什么要有它：进程表读得到命令行、读不到环境变量，而"这支进程属于哪个家"正是认领判据
 * 要的那一格 —— 同一份发布包服务多个家时（可执行件路径相同、默认端口也可能相同），
 * 只有命令行里这一句能把两个家分开。`up` 起这两个口时会带上它；
 * **手工直敲时不带** ⇒ 那一维缺席，判据退回端口（手册里的命令照旧能用，一字不改）。
 */
function homeFlag(argv) {
  const at = argv.indexOf('--home')
  if (at < 0) return null
  const value = argv[at + 1]
  return typeof value === 'string' && value.trim() !== '' ? value.trim() : null
}

async function main(argv) {
  const first = argv[0]
  if (first === 'up') return require('./scripts/home/up').up(argv.slice(1))
  if (first === 'backup') {
    // 升级/回退那条生命线的另一半（2026-09-25）：家的一次**整份**备份。它与
    // `tools/home_portability.py`（公共事实的离线备份，且不在包里）分工不同、互不取代。
    // ⚠ `--to` 缺值 = 用法错，**绝不回落成默认目的地**（真机踩到过：那会是一次没人要的整份复制）。
    const parsed = require('./scripts/home/backup').parseArgs(argv.slice(1))
    if (!parsed.ok) {
      console.log(parsed.message)
      return EXIT.USAGE
    }
    return require('./scripts/home/backup').backup({ to: parsed.to })
  }
  if (first === 'status') {
    // **`--serve` 已退役**（2026-10-01 第六刀）：两个门合成"一个进程、两个端面"⇒ 起它们那条路
    // 只有一条（`node home.js doors`）。这里**如实说**并给出去处，不静默当没看见、也不留别名
    // （留别名 = 同一件事两个入口，正是这一刀要消掉的那种重复）。
    // **不带旗标时行为一字不变**（输出、退出码、手册承诺都是既有契约）。
    if (argv.slice(1).includes('--serve')) {
      console.log('[status] `status --serve` 已退役：两个门现在是**一个进程、两个端面**（见 scripts/home/doors.js）')
      console.log('[status] 起它： node home.js doors')
      return EXIT.USAGE
    }
    return require('./scripts/home/status').status()
  }
  if (first === 'config') {
    // `config --serve` 同款退役（上面那条的理由一字不差）。不带旗标时它没有别的用法 —— 打用法。
    if (argv.slice(1).includes('--serve')) {
      console.log('[config] `config --serve` 已退役：两个门现在是**一个进程、两个端面**（见 scripts/home/doors.js）')
      console.log('[config] 起它： node home.js doors')
      return EXIT.USAGE
    }
    for (const line of USAGE) console.log(line)
    return EXIT.USAGE
  }
  if (first === 'doors') {
    // 起**家的门**（2026-10-01 第六刀）：只读状态口（读端面）＋ 模型配置门（写端面），
    // **一个进程、两个端面** —— 两个端口、两张显式路由表、两套准入（读端面可绑远程 + 家钥匙，
    // 写端面默认只本机）。为什么合并见 `scripts/home/doors.js` 的文件头。
    return require('./scripts/home/doors').serve({ homeDir: homeFlag(argv.slice(1)) })
  }
  if (first === 'remote-control') {
    // ★ **允不允许从别的机器接 Agent 到这台机器上**（2026-10-15 远程接上那一刀）。
    //
    // 为什么它是**命令**而不是一个远程可改的开关：家钥匙只回答"能不能进这台机器"，
    // 而"允许我替你起进程"是**另一件事** —— 拿到钥匙的人不该能自己授权自己
    // （判据见 `scripts/home/remote-control.js` 的文件头）。所以它只能在**这台机器上**改。
    const remoteControl = require('./scripts/home/remote-control')
    const action = (argv[1] ?? '').trim()
    if (action !== 'on' && action !== 'off') {
      for (const line of USAGE) console.log(line)
      return EXIT.USAGE
    }
    const homeDir = homeFlag(argv.slice(1)) ?? require('./scripts/home/paths').homeDir()
    const on = remoteControl.setRemoteControl(homeDir, action === 'on')
    console.log(`[remote-control] 这台机器现在${on ? '**允许**' : '**不允许**'}从别的机器接 Agent 上来（${homeDir}）`)
    console.log(
      on
        ? '[remote-control] 接上那条路仍然要家钥匙（模式二：钥匙 ＋ TLS）；这一格只管"允不允许起进程"'
        : '[remote-control] 关着的时候，外面那条路会明确回 remote_control_off（不是 404 —— 不装成不存在）',
    )
    return EXIT.OK
  }
  if (first === 'issue-home-key') {
    // 家钥匙（家口远程访问刀）：打印一次、家里只留摘要。带参数 = 用法错
    // （`up` 那种旗标面它一个都没有，别让人以为有什么隐藏开关）。
    if (argv.length > 1) {
      for (const line of USAGE) console.log(line)
      return EXIT.USAGE
    }
    return require('./scripts/home/home-key').issue()
  }
  // ⚠ 与 `up` 同一份参数纪律（2026-09-27 修）：**argv 必须传下去**。
  // 从前这一行漏了参数（`up` 那一行传了、`down` 这一行没传），而 `down` 又不解析 argv
  // ⇒ `node home.js down --help`（或任何打错的旗标）被**静默忽略**，按**默认家**跑。
  // 真机撞到过：差一点关掉主人的家（那次是空操作纯属侥幸）。这个不对称就是 bug 本身。
  // `down` 那一侧自己不认识的参数一律拒绝（`[home_down] 不认识的参数：…`，退出码 1）。
  if (first === 'down') return require('./scripts/home/down').down(argv.slice(1))
  if (first === undefined || first.startsWith('--')) {
    // 兼容 `node home.js --token` / `--status`（.sh 版的旗标形状）。
    return require('./scripts/home/up').up(argv)
  }
  for (const line of USAGE) console.log(line)
  return EXIT.USAGE
}

main(process.argv.slice(2))
  .then((code) => {
    // 用 exitCode 而不是 process.exit：让 stdout 写完再走（管道里 process.exit 会截断输出）。
    process.exitCode = code ?? 0
  })
  .catch((error) => {
    console.log(`[home] 出了个意料之外的错：${error && error.stack ? error.stack : error}`)
    process.exitCode = EXIT.USAGE
  })
