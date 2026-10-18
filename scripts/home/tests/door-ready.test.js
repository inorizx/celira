'use strict'
// **两个门的就绪判据**（2026-09-30）：回执必须是**本门的**，且起的那支进程必须还活着。
//
// 这一层治的是一条真机实测的读数谎言：两个门（只读状态口 / 模型配置门）撞同一个端口时，
// 后起的门绑定失败退出，而另一个门会把 `/healthz` / `/home-config` 答成 404 ——
// 只看"端口上有回执"就会把**别的门**当成本门起来了。
//
// 判据分四层钉住（真门对表 + 假门拒收）：
//  1. **真门对表**：真状态口 / 真配置门自己的回执必须被各自的就绪判据认下
//     （判据与门载荷两侧对齐 —— 与错误码词表"两侧对表"同一条纪律）；
//  2. **交叉拒收**：A 门应下的 404 不许被 B 门的判据认下（登记的 bug 形状）；
//  3. **waitHttp 的 match**：同一个 404 服务器 —— 不给判据 = 起来了（旧行为，im/client
//     那两支照旧）；给了本门判据 = 没起来（本刀）；
//  4. **失败三态**：它退了 / 没答 / 端口上答话的不是它 —— 三种事实分开说。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const http = require('node:http')
const os = require('node:os')
const path = require('node:path')

const up = require('../up')
const proc = require('../proc')
const { createStatusServer } = require('../status-server')
const { createConfigServer } = require('../config-server')

let cases = 0
function doorCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-door-ready-${label}-`))
}

/** 起一个监听在随机端口上的服务器；返回 `{ server, port, base }`。 */
function listen(server) {
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      resolve({ server, port: server.address().port, base: `http://127.0.0.1:${server.address().port}` })
    })
  })
}

function close(server) {
  return new Promise((resolve) => server.close(resolve))
}

/** 一个"别的门"的替身：任何请求都 404（配置门对 `/healthz` 就是这个形状）。 */
function notFoundServer() {
  return http.createServer((req, res) => {
    res.writeHead(404, { 'content-type': 'application/json; charset=utf-8' })
    res.end(JSON.stringify({ error: 'not_found' }))
  })
}

/** 一个真状态口（`/healthz` 即时答 `{ok:true}`；不碰账本 ⇒ 不需要真环境根）。 */
async function realStatusDoor() {
  const homeDir = scratch('status-home')
  const server = createStatusServer({
    homeDir,
    layout: { kind: 'source' },
    logDir: path.join(homeDir, 'logs'),
    imListen: '127.0.0.1:1',
    agentId: 'a-1',
    proc,
  })
  return listen(server)
}

/** 一个真模型配置门（`/home-config` 回三合一；空家 ⇒ state=absent）。 */
async function realConfigDoor() {
  const homeDir = scratch('config-home')
  const server = createConfigServer({ homeDir })
  return listen(server)
}

// ---------------------------------------------------------------------------
// §1 真门对表：各自的回执必须被各自的判据认下
// ---------------------------------------------------------------------------

doorCase('真状态口：`/healthz` 的回执被状态口判据认下（两侧对表）', async () => {
  const { server, base } = await realStatusDoor()
  try {
    const probe = await proc.httpProbe(`${base}/healthz`)
    assert.equal(probe.code, '200')
    assert.equal(up.statusDoorReady(probe), true, `真状态口的回执不许被拒：${probe.body}`)
  } finally {
    await close(server)
  }
})

doorCase('真配置门：`/home-config` 的回执被配置门判据认下（两侧对表）', async () => {
  const { server, base } = await realConfigDoor()
  try {
    const probe = await proc.httpProbe(`${base}/home-config`)
    assert.equal(probe.code, '200')
    assert.equal(up.configDoorReady(probe), true, `真配置门的回执不许被拒：${probe.body}`)
  } finally {
    await close(server)
  }
})

// ---------------------------------------------------------------------------
// §2 交叉拒收：A 门应下的 404 不许被 B 门认成"起来了"（登记的 bug 形状）
// ---------------------------------------------------------------------------

doorCase('交叉拒收：状态口对 `/home-config` 应 404 ⇒ 配置门判据不许认（反之亦然）', async () => {
  const statusDoor = await realStatusDoor()
  const configDoor = await realConfigDoor()
  try {
    const onStatus = await proc.httpProbe(`${statusDoor.base}/home-config`)
    assert.equal(onStatus.code, '404', '状态口的端面是穷举的：/home-config 不在其中')
    assert.equal(up.configDoorReady(onStatus), false, '别的门应下的 404 不许被当成本门就绪')

    const onConfig = await proc.httpProbe(`${configDoor.base}/healthz`)
    assert.equal(onConfig.code, '404', '配置门的端面是穷举的：/healthz 不在其中')
    assert.equal(up.statusDoorReady(onConfig), false, '别的门应下的 404 不许被当成本门就绪')
  } finally {
    await close(statusDoor.server)
    await close(configDoor.server)
  }
})

doorCase('waitHttp + match：撞格时（另一个门在应门）如实判"没起来"，而不是照抄别人的回执', async () => {
  const statusDoor = await realStatusDoor()
  try {
    // 配置门的探活打在状态口上（= 后起的门绑定失败时的那一轮）：404 ⇒ 没起来。
    const ready = await proc.waitHttp(
      [`${statusDoor.base}/home-config`],
      3,
      50,
      { match: up.configDoorReady },
    )
    assert.equal(ready.ok, false, '别的门在应门时不许判成就绪')
    assert.equal(ready.code, '404', '失败读数要带上"最后一次看到的码"（给失败原因用）')
  } finally {
    await close(statusDoor.server)
  }
})

// ---------------------------------------------------------------------------
// §3 waitHttp 的 match 语义：旧行为逐字保留（不给判据），给判据才变严
// ---------------------------------------------------------------------------

doorCase('同一个 404 服务器：不给判据 = 起来了（im/client 那两支照旧）；给了本门判据 = 没起来', async () => {
  const { server, base } = await listen(notFoundServer())
  try {
    const legacy = await proc.waitHttp([`${base}/`], 2, 50)
    assert.equal(legacy.ok, true, '旧行为：4xx 说明服务器在听（im/client 的探活靠这条）')
    assert.equal(legacy.code, '404')

    const strict = await proc.waitHttp([`${base}/healthz`], 2, 50, { match: up.statusDoorReady })
    assert.equal(strict.ok, false, '本刀：回执对不上就不算起来了')
    assert.equal(strict.code, '404', '失败时带上最后一次看到的码')

    const timeoutish = await proc.waitHttp(['http://127.0.0.1:1/'], 2, 50, { match: up.statusDoorReady })
    assert.equal(timeoutish.ok, false)
    assert.equal(timeoutish.code, '000', '一次都没答 ⇒ 000')
  } finally {
    await close(server)
  }
})

doorCase('假门：200 但不是本门的标记（回执形状不对）⇒ 两个判据都不认', async () => {
  const { server, base } = await listen(
    http.createServer((req, res) => {
      res.writeHead(200, { 'content-type': 'text/html' })
      res.end('<html>某个别的服务</html>')
    }),
  )
  try {
    const probe = await proc.httpProbe(`${base}/healthz`)
    assert.equal(probe.code, '200')
    assert.equal(up.statusDoorReady(probe), false, '200 但没有 {ok:true} 标记 ⇒ 不是状态口')
    assert.equal(up.configDoorReady(probe), false, '200 但没有三合一形状 ⇒ 不是配置门')
  } finally {
    await close(server)
  }
})

// ---------------------------------------------------------------------------
// §4 失败三态（纯函数）
// ---------------------------------------------------------------------------

doorCase('门没起来的三种事实分开说：它退了 / 没答 / 端口上答话的不是它', () => {
  assert.match(up.doorProbeFailureWhy({ alive: false, remote: false, code: '404' }), /退了/)
  assert.match(up.doorProbeFailureWhy({ alive: true, remote: false, code: '000' }), /没答/)
  assert.match(up.doorProbeFailureWhy({ alive: true, remote: true, code: undefined }), /没连上/)
  assert.match(
    up.doorProbeFailureWhy({ alive: true, remote: false, code: '404' }),
    /端口上答话的不是它（HTTP 404）/,
    '有人答话但对不上 ⇒ 如实说是"别人在答"（不是"没起来"含糊过去）',
  )
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 6, `用例数 ${cases} 少于声明的 6`)
})
