'use strict'
// 家钥匙与家口远程准入的机械判据（家口远程访问刀）。
//
// 这一刀最要紧的五条都钉在这里：
//  1. **钥匙三态 × 两口**：没带 / 不对 ⇒ 401 且码是封闭表里那一个；对了 ⇒ 照常服务；
//  2. **无会话缓存**：摘要每个请求现读 ⇒ 重新签发之后，旧钥匙**下一个请求**就 401
//     （没有"等它过期"这种机制）；
//  3. **CORS 与预检**：模式二才发 CORS 头、才答 OPTIONS；模式一**一个都不发**（回归判据）；
//  4. **fail-closed 两条**：绑非 loopback 而缺钥匙摘要 / 缺 TLS ⇒ **拒绝启动并说清缺什么**
//     （这里同时有单元判据与真实进程判据）；
//  5. **词表两侧逐字对齐**：`unauthorized` 这个码在家侧封闭表、钥匙层、客户端解码端
//     三处必须逐字相同（两侧漂移过一次的教训：`contractErrors.ts` 那段注释）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const http = require('node:http')
const https = require('node:https')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const P = require('../paths')
const homeKey = require('../home-key')
const { createStatusServer } = require('../status-server')
const configServer = require('../config-server')

let cases = 0
/** `await fn()` 而不是 `fn()`：本文件里有 async 用例（要真起 server）。漏掉 await 会把断言静默跳过。 */
function keyCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-home-key-${label}-`))
}

/** 一个"装好钥匙"的家：返回钥匙原文与家目录。 */
function homeWithKey(label, token = homeKey.generateKey()) {
  const homeDir = scratch(label)
  homeKey.writeKeyDigest(homeDir, homeKey.hashKey(token))
  return { homeDir, token }
}

// ---------------------------------------------------------------------------
// 钥匙本身
// ---------------------------------------------------------------------------

keyCase('签发：钥匙 ≥256 bit、带家钥匙前缀、不含要在头里转义的字符', () => {
  const token = homeKey.generateKey()
  assert.ok(token.startsWith(homeKey.KEY_PREFIX), `前缀不对：${token.slice(0, 8)}…`)
  const body = token.slice(homeKey.KEY_PREFIX.length)
  // base64url 的 32 字节 ⇒ 43 个字符（无 padding）⇒ 熵 ≥256 bit。
  assert.equal(body.length, 43, '32 字节的 base64url 应当正好 43 个字符')
  assert.match(body, /^[A-Za-z0-9_-]+$/, '不许出现 base64 的 + / =（它们在头/URL 里要转义）')
  assert.notEqual(homeKey.generateKey(), token, '两次签发必须是两把（CSPRNG）')
})

keyCase('摘要：SHA-256 十六进制，64 位；比对是"摘要对摘要"', () => {
  const token = homeKey.generateKey()
  const digest = homeKey.hashKey(token)
  assert.match(digest, homeKey.DIGEST_SHAPE)
  assert.equal(homeKey.hashKey(token), digest, '同一把钥匙的摘要必须一致')
  assert.equal(homeKey.keyMatches(digest, token), true)
  assert.equal(homeKey.keyMatches(digest, homeKey.generateKey()), false, '别的钥匙不许过')
  assert.equal(homeKey.keyMatches(digest, ''), false)
  assert.equal(homeKey.keyMatches('not-a-digest', token), false, '摘要形状不对 ⇒ 一律不认')
})

keyCase('落盘：文件里**只有**一行摘要（钥匙原文不落盘），重写即覆盖', () => {
  const homeDir = scratch('write')
  try {
    const first = homeKey.generateKey()
    homeKey.writeKeyDigest(homeDir, homeKey.hashKey(first))
    const text = fs.readFileSync(homeKey.keyFilePath(homeDir), 'utf8')
    assert.equal(text, `${homeKey.hashKey(first)}\n`, '文件里只有摘要那一行')
    assert.ok(!text.includes(first), '钥匙原文一个字节都不许落盘')

    // 换一把 ⇒ 旧钥匙立刻不匹配（覆盖 = 全部失效，不需要任何"过期"机制）。
    const second = homeKey.generateKey()
    homeKey.writeKeyDigest(homeDir, homeKey.hashKey(second))
    const read = homeKey.readKeyDigest(homeDir)
    assert.equal(read.state, 'present')
    assert.equal(homeKey.keyMatches(read.digest, second), true)
    assert.equal(homeKey.keyMatches(read.digest, first), false, '旧钥匙必须立刻失效')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('读摘要：不在场 / 形状不对 各自成态（"没有"与"读不出来"不是一回事）', () => {
  const homeDir = scratch('read')
  try {
    assert.equal(homeKey.readKeyDigest(homeDir).state, 'missing')
    fs.writeFileSync(homeKey.keyFilePath(homeDir), '这不是摘要\n', 'utf8')
    const malformed = homeKey.readKeyDigest(homeDir)
    assert.equal(malformed.state, 'unreadable')
    assert.equal(malformed.reason, 'malformed')
    fs.writeFileSync(homeKey.keyFilePath(homeDir), `${'a'.repeat(64)}\n`, 'utf8')
    assert.equal(homeKey.readKeyDigest(homeDir).state, 'present')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('Bearer 取值：大小写方案名都认；不是 Bearer / 没有头 ⇒ 空串', () => {
  assert.equal(homeKey.bearerOf('Bearer abc'), 'abc')
  assert.equal(homeKey.bearerOf('bearer abc'), 'abc')
  assert.equal(homeKey.bearerOf('  Bearer   abc  '), 'abc')
  assert.equal(homeKey.bearerOf('Basic abc'), '')
  assert.equal(homeKey.bearerOf(undefined), '')
})

keyCase('模式分界：127.0.0.0/8、localhost、::1 算 loopback；别的都算"出机器"', () => {
  for (const bind of ['', '127.0.0.1', '127.0.0.2', 'localhost', '::1']) {
    assert.equal(homeKey.isLoopbackBind(bind), true, `${bind || '(空)'} 应当是 loopback`)
  }
  for (const bind of ['0.0.0.0', '192.168.1.5', '10.0.0.7', 'example.internal', '::']) {
    assert.equal(homeKey.isLoopbackBind(bind), false, `${bind} 应当是"出机器"`)
  }
})

// ---------------------------------------------------------------------------
// 模式一的来源守卫（两口共用这一份实现；2026-09-25 从配置门收成一权威）
// ---------------------------------------------------------------------------

keyCase('来源守卫 · Host：loopback 的三种写法放行（端口随便），其余一律拒', () => {
  for (const host of ['127.0.0.1', '127.0.0.1:5190', 'localhost', 'localhost:5191', '[::1]', '[::1]:5190']) {
    assert.equal(homeKey.loopbackHostHeader(host), true, `${host} 应当算本机来源`)
  }
  for (const host of ['', 'evil.example', 'evil.example:5190', '127.0.0.1.evil.example', '192.168.1.20:5190', '[::2]:5190']) {
    assert.equal(homeKey.loopbackHostHeader(host), false, `${host} 不该被当成本机来源`)
  }
  // 形状不对（不是字符串）也要**判成拒**，不许抛、也不许当成"没这一维"。
  assert.equal(homeKey.loopbackHostHeader(undefined), false)
})

keyCase('来源守卫 · Origin：没有它放行（curl / 同源表单），有就必须是 loopback', () => {
  const allowed = (headers) => homeKey.localOriginAllowed({ headers })
  assert.equal(allowed({ host: '127.0.0.1:5190' }), true, '命令行 curl：没有 Origin ⇒ 可达面由绑定决定')
  assert.equal(allowed({ host: 'localhost:5190', origin: 'http://localhost:5173' }), true)
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'https://[::1]:5191' }), true)
  // 两道都要过：Host 非 loopback 就是拒（DNS rebinding 那一条），Origin 非 loopback 也是拒。
  assert.equal(allowed({ host: 'evil.example', origin: 'http://localhost:5173' }), false)
  assert.equal(allowed({ host: '127.0.0.1:5190', origin: 'https://evil.example' }), false)
  assert.equal(allowed({ host: '127.0.0.1:5190', origin: 'not a url' }), false, '读不懂的 Origin 不许放行')
})

// ★ 2026-10-05 主人裁：**壳自己的来源也算本机**。
// 为什么非有它：app 的打包形态（`tauri build`）页面来源是 Tauri 的自定义协议
// `http://tauri.localhost`，**不是 loopback** ⇒ 配置门会把自己的页面拒掉 —— 真机实测，
// 打包形态里机器页「模型」那一间读不到（"这个来源不被允许"），而 dev 形态（页面在
// `localhost:5173`）一直是绿的（`app/README.md` §5 第 10 行）。
keyCase('★ 来源守卫 · 壳自己的来源（Tauri 自定义协议）放行；别的域名照旧一律拒', () => {
  const allowed = (headers) => homeKey.localOriginAllowed({ headers })
  // 正例：两种 webview 写法（Windows/Linux 的 `http://tauri.localhost`、macOS 的 `tauri://localhost`）。
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'http://tauri.localhost' }), true)
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'tauri://localhost' }), true)
  // 反例（这一条才是"没放宽"的证据）：别的来源照旧拒，哪怕它长得像、哪怕它指向 loopback 的名字。
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'http://tauri.localhost.evil.example' }), false)
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'https://evil.example' }), false)
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'tauri://evil.example' }), false, '壳的来源里主机也要对')
  assert.equal(allowed({ host: '127.0.0.1:5191', origin: 'file:///c:/x.html' }), false)
  // Host 那一道一个字没动：Host 非 loopback 时，壳自己的来源也过不去。
  assert.equal(allowed({ host: 'tauri.localhost:5191', origin: 'http://tauri.localhost' }), false)
})

// ---------------------------------------------------------------------------
// fail-closed 两条（开口之前拒，并说清缺什么）
// ---------------------------------------------------------------------------

keyCase('fail-closed：绑非 loopback 而家里没有钥匙摘要 ⇒ 拒，并点名 issue-home-key', () => {
  const homeDir = scratch('nokey')
  try {
    const launch = homeKey.resolveLaunch({ bind: '0.0.0.0', homeDir, tlsCert: '', tlsKey: '' })
    assert.equal(launch.ok, false)
    assert.match(launch.message, /issue-home-key/, '要告诉人怎么补钥匙')
    assert.match(launch.message, /0\.0\.0\.0/, '要说清是哪个绑定把它变成了远程')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('fail-closed：有钥匙但缺 TLS ⇒ 拒，并**点名缺哪一份**（证书 / 私钥分开说）', () => {
  const { homeDir } = homeWithKey('nocert')
  try {
    const none = homeKey.resolveLaunch({ bind: '192.168.1.5', homeDir, tlsCert: '', tlsKey: '' })
    assert.equal(none.ok, false)
    assert.match(none.message, /tls_cert/)
    assert.match(none.message, /tls_key/)

    const half = homeKey.resolveLaunch({
      bind: '192.168.1.5',
      homeDir,
      tlsCert: 'D:/nowhere/cert.pem',
      tlsKey: '',
    })
    assert.equal(half.ok, false)
    assert.match(half.message, /tls_key/, '只缺私钥就只说私钥')
    assert.ok(!half.message.includes('tls_cert 与'), '不许把缺的那份说成两份都缺')

    const gone = homeKey.resolveLaunch({
      bind: '192.168.1.5',
      homeDir,
      tlsCert: 'D:/nowhere/cert.pem',
      tlsKey: 'D:/nowhere/key.pem',
    })
    assert.equal(gone.ok, false)
    assert.match(gone.message, /D:\/nowhere\/cert\.pem/, '文件不在场时要说清是哪个路径')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('齐了才放行：非 loopback + 钥匙 + TLS 两份文件 ⇒ 远程模式；loopback ⇒ 模式一（配了也当没配）', () => {
  const { homeDir } = homeWithKey('ok')
  fs.writeFileSync(path.join(homeDir, 'cert.pem'), 'cert-bytes', 'utf8')
  fs.writeFileSync(path.join(homeDir, 'key.pem'), 'key-bytes', 'utf8')
  try {
    const remote = homeKey.resolveLaunch({
      bind: '0.0.0.0',
      homeDir,
      tlsCert: path.join(homeDir, 'cert.pem'),
      tlsKey: path.join(homeDir, 'key.pem'),
    })
    assert.equal(remote.ok, true)
    assert.equal(remote.access.mode, 'remote')
    assert.equal(remote.access.homeDir, homeDir)
    assert.equal(remote.tls.cert.toString('utf8'), 'cert-bytes')

    const local = homeKey.resolveLaunch({
      bind: '127.0.0.1',
      homeDir,
      tlsCert: path.join(homeDir, 'cert.pem'),
      tlsKey: path.join(homeDir, 'key.pem'),
    })
    assert.equal(local.ok, true)
    assert.equal(local.access.mode, 'local', '绑 loopback 就是模式一')
    assert.equal(local.tls, null, '模式一不读证书（两模式不混搭）')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('fail-closed（真实进程）：node home.js doors 绑非本机 + 缺钥匙 ⇒ 退出码 3 且说清缺什么', () => {
  const homeDir = scratch('spawn')
  try {
    fs.writeFileSync(
      path.join(homeDir, 'home.conf'),
      'status_bind=0.0.0.0\nstatus_port=5190\n',
      'utf8',
    )
    const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'doors'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 20000,
      env: {
        SystemRoot: process.env.SystemRoot,
        windir: process.env.windir,
        PATH: process.env.PATH,
        CELIRA_HOME_DIR: homeDir,
      },
    })
    assert.equal(done.status, 3, `应当以"需要你动手"退出（实际 ${done.status}）：${done.stdout}${done.stderr}`)
    assert.match(done.stdout, /issue-home-key/)
    assert.match(done.stdout, /钥匙/)

    // 同一个家，把钥匙补上、并把配置门也绑到非本机：这次的拒因换成"缺 TLS"
    // （两条 fail-closed 各自可判；⚠ 两口各读各的绑定键 —— 只写 status_bind 的话，
    //   config 那一口还是模式一，会**正常常驻**，这条进程判据就变成了在等一个服务）。
    homeKey.writeKeyDigest(homeDir, homeKey.hashKey(homeKey.generateKey()))
    fs.writeFileSync(
      path.join(homeDir, 'home.conf'),
      'status_bind=0.0.0.0\nstatus_port=5190\nconfig_bind=0.0.0.0\nconfig_port=5191\n',
      'utf8',
    )
    const second = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'doors'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 20000,
      env: {
        SystemRoot: process.env.SystemRoot,
        windir: process.env.windir,
        PATH: process.env.PATH,
        CELIRA_HOME_DIR: homeDir,
      },
    })
    assert.equal(second.status, 3)
    assert.match(second.stdout, /tls_cert/)
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

keyCase('签发命令（真实进程）：只打印一次、家里只留摘要；重跑换新的一把', () => {
  const homeDir = scratch('issue')
  const env = {
    SystemRoot: process.env.SystemRoot,
    windir: process.env.windir,
    PATH: process.env.PATH,
    CELIRA_HOME_DIR: homeDir,
  }
  const run = () =>
    spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'issue-home-key'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 20000,
      env,
    })
  try {
    const first = run()
    assert.equal(first.status, 0)
    const line = first.stdout.split(/\r?\n/).find((each) => each.startsWith('[home-key] '))
    assert.ok(line, `没打印钥匙那一行：${first.stdout}`)
    const token = line.slice('[home-key] '.length).trim()
    assert.match(first.stdout, /只显示这一次/)
    assert.match(first.stdout, /旧钥匙立刻全部失效/)

    const read = homeKey.readKeyDigest(homeDir)
    assert.equal(read.state, 'present')
    assert.equal(homeKey.keyMatches(read.digest, token), true, '打印出来的那一把就是生效的那一把')
    const fileText = fs.readFileSync(homeKey.keyFilePath(homeDir), 'utf8')
    assert.ok(!fileText.includes(token), '钥匙原文不许落盘')

    const second = run()
    const token2 = second.stdout
      .split(/\r?\n/)
      .find((each) => each.startsWith('[home-key] '))
      .slice('[home-key] '.length)
      .trim()
    assert.notEqual(token2, token, '重跑 = 换一把新的')
    const after = homeKey.readKeyDigest(homeDir)
    assert.equal(homeKey.keyMatches(after.digest, token2), true)
    assert.equal(homeKey.keyMatches(after.digest, token), false, '旧钥匙立刻失效')
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 两口 × 钥匙三态（真起 server）
// ---------------------------------------------------------------------------

/** 发一个请求（带可选 Authorization）；`protocol` 给 'https' 时用 https 模块。 */
function call(port, method, target, { headers, protocol = 'http' } = {}) {
  const mod = protocol === 'https' ? https : http
  return new Promise((resolve, reject) => {
    const req = mod.request(
      { host: '127.0.0.1', port, method, path: target, headers: headers ?? {}, rejectUnauthorized: false },
      (res) => {
        let text = ''
        res.setEncoding('utf8')
        res.on('data', (chunk) => {
          text += chunk
        })
        res.on('end', () => {
          let json = null
          try {
            json = JSON.parse(text)
          } catch {
            json = null
          }
          resolve({ status: res.statusCode, headers: res.headers, text, json })
        })
      },
    )
    req.on('error', reject)
    req.end()
  })
}

/** 起一个口（status 或 config），跑完一定关掉。 */
async function withRemoteServer(which, run, { token, digestFile } = {}) {
  const homeDir = scratch(`remote-${which}`)
  const key = token ?? homeKey.generateKey()
  if (digestFile !== null) homeKey.writeKeyDigest(homeDir, homeKey.hashKey(key))
  const access = { mode: 'remote', homeDir }
  const server =
    which === 'status'
      ? createStatusServer(
          {
            homeDir,
            layout: { statusExe: 'D:/nowhere/agent_host_status.exe' },
            logDir: path.join(homeDir, 'logs'),
            imListen: '127.0.0.1:1',
            agentId: 'agent-1',
            proc: { httpCode: async () => 0, isFile: () => false },
          },
          { access, ttlMs: 1 },
        )
      : configServer.createConfigServer(
          {
            homeDir,
            probe: async (endpoint) => ({
              url: configServer.modelsUrl(endpoint),
              ok: true,
              status: 200,
              reason: null,
              contextWindow: null,
              windowField: null,
              models: [],
            }),
          },
          { access },
        )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  try {
    await run({ port, homeDir, token: key })
  } finally {
    await new Promise((resolve) => server.close(resolve))
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
}

const AUTHORIZED = (token) => ({ authorization: `Bearer ${token}` })

keyCase('状态口 × 钥匙三态：没带 / 不对 ⇒ 401 unauthorized；对了 ⇒ 200', async () => {
  await withRemoteServer('status', async ({ port, token }) => {
    const none = await call(port, 'GET', '/status')
    assert.equal(none.status, 401)
    assert.equal(none.json.error, homeKey.UNAUTHORIZED)

    const wrong = await call(port, 'GET', '/status', {
      headers: AUTHORIZED(homeKey.generateKey()),
    })
    assert.equal(wrong.status, 401)
    assert.equal(wrong.json.error, homeKey.UNAUTHORIZED)

    const right = await call(port, 'GET', '/status', { headers: AUTHORIZED(token) })
    assert.equal(right.status, 200)
    assert.ok(right.json.readAt, '读数照常给（认证不改变内容）')

    // 端面其余两条同样在钥匙后面（/healthz 不是后门）。
    assert.equal((await call(port, 'GET', '/healthz')).status, 401)
    assert.equal((await call(port, 'GET', '/healthz', { headers: AUTHORIZED(token) })).status, 200)
  })
})

keyCase('配置门 × 钥匙三态：读 / 写两条路都是 401 或 200', async () => {
  await withRemoteServer('config', async ({ port, token }) => {
    assert.equal((await call(port, 'GET', '/home-config')).status, 401)
    assert.equal(
      (await call(port, 'GET', '/home-config', { headers: AUTHORIZED(homeKey.generateKey()) })).status,
      401,
    )
    const read = await call(port, 'GET', '/home-config', { headers: AUTHORIZED(token) })
    assert.equal(read.status, 200)
    assert.equal(read.json.state, 'absent', '家还没配模型 ⇒ 如实说没配')

    // 写那条路同样在钥匙后面（没钥匙的 PUT 不许碰文件）。
    const put = await call(port, 'PUT', '/home-binding', {
      headers: { 'content-type': 'application/json' },
    })
    assert.equal(put.status, 401)
  })
})

keyCase('无会话缓存：重新签发之后，旧钥匙**下一个请求**就 401', async () => {
  await withRemoteServer('config', async ({ port, token, homeDir }) => {
    assert.equal((await call(port, 'GET', '/home-config', { headers: AUTHORIZED(token) })).status, 200)
    // 换钥匙（重跑 issue-home-key 的实质：覆盖摘要文件）。
    const next = homeKey.generateKey()
    homeKey.writeKeyDigest(homeDir, homeKey.hashKey(next))
    assert.equal(
      (await call(port, 'GET', '/home-config', { headers: AUTHORIZED(token) })).status,
      401,
      '摘要每请求现读 ⇒ 旧钥匙不许再多活一个请求',
    )
    assert.equal((await call(port, 'GET', '/home-config', { headers: AUTHORIZED(next) })).status, 200)
  })
})

keyCase('摘要被删掉 ⇒ 一律 401（fail-closed，不许回落到"免钥匙"）', async () => {
  await withRemoteServer('status', async ({ port, token, homeDir }) => {
    assert.equal((await call(port, 'GET', '/status', { headers: AUTHORIZED(token) })).status, 200)
    fs.rmSync(homeKey.keyFilePath(homeDir), { force: true })
    assert.equal((await call(port, 'GET', '/status', { headers: AUTHORIZED(token) })).status, 401)
  })
})

// ---------------------------------------------------------------------------
// CORS 与预检
// ---------------------------------------------------------------------------

keyCase('模式二：响应带 CORS 头（含 401 那份），预检 204 且只放行两个头', async () => {
  await withRemoteServer('config', async ({ port, token }) => {
    const denied = await call(port, 'GET', '/home-config')
    assert.equal(denied.headers['access-control-allow-origin'], '*', '401 也要带 CORS 头，否则浏览器读不到它')

    const preflight = await call(port, 'OPTIONS', '/home-config', {
      headers: {
        origin: 'http://localhost:5173',
        'access-control-request-method': 'PUT',
        'access-control-request-headers': 'authorization, content-type',
      },
    })
    assert.equal(preflight.status, 204, '预检不用钥匙（规范就是不带 Authorization）')
    assert.equal(preflight.headers['access-control-allow-origin'], '*')
    assert.match(preflight.headers['access-control-allow-methods'], /PUT/)
    assert.equal(
      preflight.headers['access-control-allow-headers'],
      'authorization, content-type',
      '只列实际会用的两个头（不写通配）',
    )

    const ok = await call(port, 'GET', '/home-config', { headers: AUTHORIZED(token) })
    assert.equal(ok.headers['access-control-allow-origin'], '*')
  })
})

keyCase('回归：模式一（local）一个 CORS 头都不发、OPTIONS 照旧 405', async () => {
  const homeDir = scratch('local-cors')
  try {
    const server = createStatusServer(
      {
        homeDir,
        layout: { statusExe: 'D:/nowhere/none.exe' },
        logDir: homeDir,
        imListen: '127.0.0.1:1',
        agentId: 'agent-1',
        proc: { httpCode: async () => 0, isFile: () => false },
      },
      { ttlMs: 1 },
    )
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
    const port = server.address().port
    try {
      const res = await call(port, 'GET', '/status', { headers: { origin: 'http://localhost:5173' } })
      assert.equal(res.status, 200)
      assert.equal(res.headers['access-control-allow-origin'], undefined)
      assert.equal(res.headers.vary, undefined)
      const options = await call(port, 'OPTIONS', '/status')
      assert.equal(options.status, 405, '模式一里 OPTIONS 仍然是 405（与从前一字不差）')
    } finally {
      await new Promise((resolve) => server.close(resolve))
    }
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 词表两侧逐字对齐
// ---------------------------------------------------------------------------

keyCase('词表：unauthorized 在家侧封闭表、钥匙层、客户端解码端三处逐字相同', () => {
  assert.equal(homeKey.UNAUTHORIZED, 'unauthorized')
  assert.equal(configServer.ERRORS.UNAUTHORIZED, homeKey.UNAUTHORIZED, '封闭表引用钥匙层那一份')
  const clientSource = fs.readFileSync(
    path.join(P.REPO, 'client', 'src', 'api', 'homeConfig.ts'),
    'utf8',
  )
  assert.ok(
    clientSource.includes(`'${homeKey.UNAUTHORIZED}'`),
    '客户端那份封闭表里必须有同一个码（改一边忘另一边就是契约漂移）',
  )
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '家钥匙用例数必须大于零')
  console.log(`      home-key 用例数：${cases}`)
})