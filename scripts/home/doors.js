'use strict'
/**
 * **家的门**：只读状态口（读端面）＋ 模型配置门（写端面）—— **一个进程、两个端面**。
 *
 * 为什么合成一个进程（2026-10-01 第六刀，主人裁："别被以前的文档束缚，要合理"）：
 *   · 两个门服务的是**同一个读者**（本机页面，走同源代转）、**同一个寿命**（页面在不在跑）、
 *     **同一句恢复话**（重开一次 `node home.js up`）。分开只买到"一个崩了另一个还在"，
 *     而用户能做的还是同一件事 —— 代价却是每个家多一个 Node 进程（实测 ≈56 MB）＋
 *     **一整套重复的**启动 / 探活 / 账本 / 日志 / 关停 / 归属判据（`proc.js` 里那两条
 *     跨词短语 matcher 就是这套重复的化石：它们要互相提防"认错进程会去杀错人"）；
 *   · **合并的是"谁在听"，不是"谁能被谁访问"**：端口、路由、访问控制、TLS/家钥匙
 *     **一条不合并**（两个端面各自 `resolveLaunch`、各自绑定、各自一张显式路由表）。
 *
 * 保住的两条故障纪律：
 *   ① **各自绑定**：一个端面绑不上（端口被占）不许把另一个也拖下水 —— 读数点名是谁、为什么；
 *   ② **不许串门**：读端面只有 `GET /status` 与 `GET /healthz`；写端面只有那几条配置路由。
 *      同一个进程**不是**"读口顺带能写"的理由（路由表逐条列出，不靠前缀、不靠进程边界）。
 *
 * 与读者的关系：**读者永远是"家的外面"**（本机页面经 `scripts/serve_client.mjs` 同源代转，
 * 或远端经家通道 TLS ＋ 家钥匙）。这个进程不读页面、不读浏览器，只回答两个端面上的请求。
 */
const path = require('node:path')

const P = require('./paths')
const { confGet } = require('./conf')
const { EXIT } = require('./codes')
const homeKey = require('./home-key')
const modelSources = require('./model-sources')
const residents = require('./residents')
const statusServer = require('./status-server')
const configServer = require('./config-server')

const DEFAULT_STATUS_PORT = P.DEFAULT_STATUS_PORT
const DEFAULT_CONFIG_PORT = P.DEFAULT_CONFIG_PORT

/**
 * 两个端面的**规划**（纯函数：只读家的配置，不起任何东西、不碰网络）。
 *
 * 用例直接喂一份 `home.conf` 断言它 —— "哪个端面开在哪个口、谁需要钥匙"是**家的配置事实**，
 * 不该只有起进程才看得出。
 */
function planEndfaces({ homeDir, confFile } = {}) {
  const home = P.homeDir(homeDir)
  const file = confFile ?? path.join(home, 'home.conf')
  const plans = []

  for (const spec of [
    { kind: 'status', label: '只读状态口', portKey: 'status_port', bindKey: 'status_bind', fallbackPort: DEFAULT_STATUS_PORT },
    { kind: 'config', label: '模型配置门', portKey: 'config_port', bindKey: 'config_bind', fallbackPort: DEFAULT_CONFIG_PORT },
  ]) {
    const rawPort = confGet(file, spec.portKey, spec.fallbackPort)
    const port = Number.parseInt(rawPort, 10)
    const bind = (confGet(file, spec.bindKey, homeKey.DEFAULT_BIND) || '').trim() || homeKey.DEFAULT_BIND
    if (!Number.isInteger(port) || port <= 0 || port > 65535) {
      plans.push({ ...spec, port: null, bind, error: `home.conf 里的 ${spec.portKey} 不是合法端口：${rawPort}` })
      continue
    }
    const launch = homeKey.resolveLaunch({
      bind,
      homeDir: home,
      tlsCert: confGet(file, 'tls_cert'),
      tlsKey: confGet(file, 'tls_key'),
    })
    plans.push({ ...spec, port, bind, launch })
  }

  return { homeDir: home, confFile: file, endfaces: plans }
}

/**
 * 起两个端面并**常驻**（`node home.js doors` 的入口）。
 *
 * `options.homeDir` = 命令行里声明的那一份（`--home`，`up` 起它时会带；手工直敲不带）。
 */
async function serve(options = {}) {
  const plan = planEndfaces(options)

  // 先核两个端面的"能不能开口"：**不合起来判**（一个端面的坏消息不许掩盖另一个的）。
  let blocked = false
  for (const endface of plan.endfaces) {
    if (endface.error) {
      console.log(`[doors] ${endface.label}：${endface.error}`)
      blocked = true
    } else if (!endface.launch.ok) {
      console.log(`[doors] ${endface.label}：${endface.launch.message}`)
      blocked = true
    }
  }
  if (blocked) return EXIT.NEEDS_YOU

  const confFile = plan.confFile
  const homeDir = plan.homeDir
  const logDir = path.join(homeDir, 'logs')
  const imListen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)
  const agentId = confGet(confFile, 'agent_id')
  // 住户表：**启动时取一份只为下面那次老形状迁移**（那是一次开机动作）；
  // 两个端面拿到的是**函数**，每次请求现问 —— 门起来之后再安的那一支才算数
  // （2026-10-12：从前这里把同一份数组交给两个端面，它到门退出为止都不会变）。
  const residentList = residents.runtimeResidents(homeDir, confFile)
  const residentsOf = () => residents.runtimeResidents(homeDir, confFile)

  console.log('[doors] 家的门（一个进程，两个端面）：')

  // ---------- 写端面（模型配置门）：先把老形状迁了，再开口 ----------
  // **开机先看一眼要不要迁移**（刀2b）：老的单份 `<环境根>/model_source.json` ⇒ 清单第一条 + 绑定。
  // 幂等，所以与 `up.js` 里那次重跑无害；`conflict`（老文件与绑定同时在场）**不自动选边**，如实报。
  // ⚠ 住户化之后**一支一支看**（一支一个环境根；还没迁的老家那一支的环境根仍是 `<家>/env`）。
  for (const resident of residentList) {
    const migration = modelSources.migrateIfNeeded(homeDir, resident.envDir)
    if (migration.action === 'migrated') {
      console.log(`[doors] 迁移（住户 ${resident.agentId}）：${migration.detail}（清单条目 ${migration.entry?.id}）`)
    } else if (migration.action === 'conflict' || migration.action === 'failed') {
      console.log(`[doors] ⚠ 迁移没做（住户 ${resident.agentId}）：${migration.detail}`)
    }
  }

  const listeners = []

  const statusPlan = plan.endfaces.find((e) => e.kind === 'status')
  const statusServerInstance = statusServer.createStatusServer(
    {
      homeDir,
      layout: P.detectLayout(),
      logDir,
      imListen,
      // **住户表**（一支一份读数）：还没住户化的老家那一支也在这里面（`runtimeResidents`）。
      // 它是**函数**：每次请求现问（见上面那段注释）。
      residents: residentsOf,
      agentId,
      proc: require('./proc'),
    },
    { access: statusPlan.launch.access, tls: statusPlan.launch.tls },
  )

  const configPlan = plan.endfaces.find((e) => e.kind === 'config')
  const configServerInstance = configServer.createConfigServer(
    {
      homeDir,
      // **住户表**（绑定落在每一支的环境根上）：还没住户化的老家那一支也在里面。
      // 同上是**函数**：换绑那几笔写也要落在"此刻真的住着的每一支"上。
      residents: residentsOf,
    },
    { access: configPlan.launch.access, tls: configPlan.launch.tls },
  )

  for (const [endface, server, where] of [
    [statusPlan, statusServerInstance, `/status`],
    [configPlan, configServerInstance, `/home-config`],
  ]) {
    try {
      await new Promise((resolve, reject) => {
        server.once('error', reject)
        server.listen(endface.port, endface.bind, resolve)
      })
      listeners.push({ endface, server })
    } catch (error) {
      // ① 各自绑定：这一条失败**不许**把另一条拖下水（另一个端面照旧服务）。
      console.log(
        `[doors] ${endface.label} 起不来（端口 ${endface.port} 被占？）：${error && error.message ? error.message : error}`,
      )
    }
  }

  if (listeners.length === 0) {
    console.log('[doors] 两个端面都没起来 —— 一个口都没开')
    return 1
  }

  for (const { endface } of listeners) {
    const scheme = endface.launch.tls ? 'https' : 'http'
    const port = endface.port
    const host = endface.launch.tls ? endface.bind : '127.0.0.1'
    if (endface.kind === 'status') {
      console.log(`[doors] 读端面（只读状态口）：${scheme}://${host}:${port}/status`)
      console.log('        端面只有两个：GET /status（本家读数）、GET /healthz（活着）')
      console.log(`        TTL ${Math.round(statusServer.STATUS_TTL_MS / 1000)} 秒；开机先热一次 ⇒ 页面第一眼不必等 3~4 秒`)
    } else {
      console.log(`[doors] 写端面（模型配置门）：${scheme}://${host}:${port}/home-config`)
      console.log('        端面：/home-config（GET 读 / POST 新建 / PUT 改 / DELETE 删）、')
      console.log('              /home-binding（PUT 换绑）、/home-config/probe（POST 只读探测）')
    }
    if (endface.launch.tls) {
      console.log('        **远程模式**：进门要带家钥匙（Authorization: Bearer …；issue-home-key 那一步给过）')
      console.log('        换钥匙 = 重跑 node home.js issue-home-key（旧钥匙下一请求即失效）')
    }
  }
  console.log('        口上没有家的凭据；两个端面的准入各判各的（读端面可远程、写端面默认只本机）')
  console.log('        Ctrl-C 停')

  // **开机预热**（TASK_72 ③）：一次探测 3.7 秒，而页面进来就要读数 —— 先在后台探一次，
  // 页面第一次问时直接命中缓存。它不阻塞启动（`warmStatus` 内部是 fire-and-forget）。
  if (listeners.some((l) => l.endface.kind === 'status')) statusServerInstance.warmStatus()

  // 常驻：这是服务不是一次性命令 —— 返回一个永不 settle 的 promise，
  // 让 `home.js` 的收尾逻辑不执行（也就不会打印退出码）。
  return new Promise(() => {})
}

module.exports = {
  planEndfaces,
  serve,
  DEFAULT_STATUS_PORT,
  DEFAULT_CONFIG_PORT,
}
