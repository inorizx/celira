'use strict'
// 模型配置门与它的机械判据（TASK_77；刀2b 起管"清单 + 绑定"两个事实）。
//
// 这一件最要紧的六条都在这里钉住：
//  1. **端面穷举**：`/home-config`（GET / POST / PUT / DELETE）、`/home-binding`（PUT）与
//     `/home-config/probe`（POST），其余 404、别的动词 405；
//  2. **口上无秘密**：`GET` 永不回 key（只有 `keyConfigured` 一个布尔）；响应里**没有任何
//     CORS 头**；口的进程环境里没有家的凭据；
//  3. **来源两道（按模式分）**：**模式一**里 `Host` 与 `Origin` 都只认 loopback
//     （DNS rebinding 那一道不是装饰）；**模式二**里浏览器必然跨源，闸是家钥匙 + TLS ——
//     再按 loopback 卡 `Host` 会把"跨机改模型配置"整条路堵死（2026-09-25 真机踩到）；
//  4. **保存 ≠ 能连**：试连失败**不拦保存**，但读数要如实（含 HTTP 状态码那一格）；
//  5. **清单归机器，绑定归 Agent**（刀2b）：两个文件各管各的、迁移幂等、悬空绑定如实拒绝、
//     被绑定挡住的删除要**拒**而不是静默删；
//  6. **探测只读**（2026-09-23 预设供应商层）：`POST /home-config/probe` 复用同一个试连，
//     **一个字节都不落盘**、响应里没有 key、试连失败是读数（200 + `ok:false`）不是错误。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const http = require('node:http')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const {
  createConfigServer,
  probeModels,
  modelsUrl,
  windowFromModels,
  modelsFromBody,
  WINDOW_FIELDS,
  MODEL_LIST_LIMIT,
  PROBE_PATH,
  CATALOG_PATH,
  ERRORS,
  serveEnv,
  modelSources,
} = require('../config-server')
const { createWindowTable } = require('../model-windows')
const homeKey = require('../home-key')
// ★ 见证那一本（2026-10-18）：门改过那两份文件就记一条，读数里**两档分开**报。
const modelWitness = require('../model_witness')

/** 探针读数里"没探到窗口"那一档（形状里那两格：必须给，可以是 null）。 */
const NO_WINDOW = { contextWindow: null, windowField: null }
/** 探针读数的**整份**形状（刀2b 起还有模型目录那一格：读不到就是空数组）。 */
const NO_CATALOG = { ...NO_WINDOW, models: [] }

let cases = 0
/** `await fn()` 而不是 `fn()`：本文件里有 async 用例（要真起一个 server）。漏掉 await 的用例
 *  断言会被静默跳过、还假装通过 —— 那正是"测试空跑"。 */
function configCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一次性环境根（照本仓既有测试的临时目录约定）。 */
function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-config-gate-${label}-`))
}

/** 这个门的两个文件（形状的唯一权威在 `model-sources.js`，用例也用它，不另抄一份路径）。 */
function listFile(homeDir) {
  return modelSources.listPath(homeDir)
}
function bindingFile(homeDir) {
  return modelSources.bindingPath(path.join(homeDir, 'env'))
}
/** 直接读清单（判"落盘的是什么"时用），读成数组（新形状 `{next, entries}` / 老数组都认）。 */
function readList(homeDir) {
  const parsed = JSON.parse(fs.readFileSync(listFile(homeDir), 'utf8'))
  return Array.isArray(parsed) ? parsed : parsed.entries
}
/** 直接读绑定（判"绑到哪一条"时用）。 */
function readBinding(homeDir) {
  return JSON.parse(fs.readFileSync(bindingFile(homeDir), 'utf8'))
}
/** 老的单份配置（迁移的输入）。 */
function writeLegacy(homeDir, text) {
  fs.mkdirSync(path.join(homeDir, 'env'), { recursive: true })
  fs.writeFileSync(path.join(homeDir, 'env', 'model_source.json'), text, 'utf8')
}

/** 起一个**真的** server（绑 127.0.0.1 的临时端口），跑完一定关掉。
 *  `options` 直通 `createConfigServer` 的第二参（`access` / `tls`）——模式二的用例靠它。 */
async function withServer(fn, ctx = {}, options = {}) {
  const probes = []
  const server = createConfigServer(
    {
      ...ctx,
      probe:
        ctx.probe ??
        (async (endpoint, credential) => {
          probes.push({ endpoint, credential })
          return {
            url: modelsUrl(endpoint),
            ok: true,
            status: 200,
            reason: null,
            ...NO_CATALOG,
          }
        }),
    },
    options,
  )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  try {
    await fn({ port, probes, server })
  } finally {
    await new Promise((resolve) => server.close(resolve))
  }
}

/** 真发一个请求（`Host` 可以按用例改写——那正是要测的那一道）。 */
function request(port, method, target, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const payload = body === undefined ? null : Buffer.from(JSON.stringify(body), 'utf8')
    const requestHeaders = { 'content-type': 'application/json', ...headers }
    if (payload) requestHeaders['content-length'] = payload.length
    const req = http.request(
      { host: '127.0.0.1', port, method, path: target, headers: requestHeaders },
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
    if (payload) req.write(payload)
    req.end()
  })
}

/** 一份合法的 PUT 体。 */
function body(overrides = {}) {
  return {
    endpoint: 'https://api.example.invalid/chat/completions',
    apiKey: 'sk-secret-value',
    model: 'some-model',
    contextWindow: 131072,
    ...overrides,
  }
}

// ---------------------------------------------------------------------------
// 端面
// ---------------------------------------------------------------------------

configCase('端面穷举：只有 /home-config、/home-binding、/home-config/probe、/home-config/catalog、/home-control/attach 五条，别的一律 404', async () => {
  const root = scratch('routes')
  try {
    await withServer(async ({ port }) => {
      assert.equal((await request(port, 'GET', '/home-config')).status, 200)
      for (const other of ['/', '/status', '/home-config/x', '/home-binding/x', '/healthz', '/probe']) {
        const res = await request(port, 'GET', other)
        assert.equal(res.status, 404, `${other} 应当是 404`)
        assert.equal(res.json.error, ERRORS.NOT_FOUND)
      }
      // 绑定的**读**面不开第二条：读绑定走在 GET /home-config 里。
      const bindingRead = await request(port, 'GET', '/home-binding')
      assert.equal(bindingRead.status, 405)
      assert.equal(bindingRead.headers.allow, 'PUT')
      // 探测只收 POST（key 走请求体，不进 URL）——GET 它是 405，不是 404。
      const probeRead = await request(port, 'GET', PROBE_PATH)
      assert.equal(probeRead.status, 405)
      assert.equal(probeRead.headers.allow, 'POST')
      // 目录那一格也只收 POST（条目 id 走请求体）——2026-10-14 加。
      const catalogRead = await request(port, 'GET', CATALOG_PATH)
      assert.equal(catalogRead.status, 405)
      assert.equal(catalogRead.headers.allow, 'POST')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('端面穷举：四条之外的动词一律 405，且 Allow 如实列出它认的四条', async () => {
  const root = scratch('methods')
  try {
    await withServer(async ({ port }) => {
      const res = await request(port, 'PATCH', '/home-config', { body: {} })
      assert.equal(res.status, 405)
      assert.equal(res.json.error, ERRORS.METHOD_NOT_ALLOWED)
      assert.equal(res.headers.allow, 'GET, POST, PUT, DELETE')
      // `/home-binding` 只收 PUT（换绑 / 解绑），POST 到它那里也是 405。
      const posted = await request(port, 'POST', '/home-binding', { body: { source: 'ms-1' } })
      assert.equal(posted.status, 405)
      assert.equal(posted.headers.allow, 'PUT')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('**不发任何 CORS 头**：同源代转是唯一通路，跨源写由浏览器自己拦', async () => {
  const root = scratch('cors')
  try {
    await withServer(async ({ port }) => {
      for (const [method, target] of [
        ['GET', '/home-config'],
        ['GET', '/home-binding'],
        ['DELETE', '/home-config'],
      ]) {
        const res = await request(port, method, target)
        const cors = Object.keys(res.headers).filter((key) => key.startsWith('access-control-'))
        assert.deepEqual(cors, [], `${method} 的响应里不该有 CORS 头`)
      }
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 来源两道
// ---------------------------------------------------------------------------

configCase('来源第二道：Host 不是 loopback ⇒ 403（这一条才是防 DNS rebinding 的）', async () => {
  const root = scratch('host')
  try {
    await withServer(async ({ port }) => {
      const res = await request(port, 'GET', '/home-config', { headers: { host: 'evil.example' } })
      assert.equal(res.status, 403)
      assert.equal(res.json.error, ERRORS.ORIGIN_NOT_ALLOWED)
      // 负向对照：loopback 的 Host 照常放行。
      assert.equal((await request(port, 'GET', '/home-config')).status, 200)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('来源第二道：Origin 有就必须是 loopback（localhost 的页面走代转照常放行）', async () => {
  const root = scratch('origin')
  try {
    await withServer(async ({ port }) => {
      const denied = await request(port, 'PUT', '/home-config', {
        body: body(),
        headers: { origin: 'https://evil.example' },
      })
      assert.equal(denied.status, 403)
      const allowed = await request(port, 'PUT', '/home-config', {
        body: body(),
        headers: { origin: 'http://localhost:5173' },
      })
      assert.equal(allowed.status, 200, allowed.text)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★ 来源那道排在路由之前：外域来源打 PUT-only 端点 ⇒ 403（**不是** 405，也不给 Allow）', async () => {
  // 判据：**与壳那一侧同形**（`app/src-tauri/src/home_doors.rs` 先判来源、再进路由）。
  // 从前这里先判方法 ⇒ 外域来源拿到 405 + `Allow: PUT`（把"端点存在、认哪些动词"告诉了一个
  // 不该知道的来源），而壳那边给 403 ✗ —— 两种答案，取决于哪个生产者占着那两个口。
  // 真机门禁有一条同形断言（`verify-ui-app.mjs`：外域来源打 `/home-binding` ⇒ 403），
  // 今天它是**先红的**：那次答话的是源码形态的门 ✗ ⇒ 这一条把它钉在单元级。
  const root = scratch('origin-before-route')
  try {
    await withServer(async ({ port }) => {
      const foreign = await request(port, 'GET', '/home-binding', {
        headers: { origin: 'https://evil.example' },
      })
      assert.equal(foreign.status, 403, '外域来源必须先撞来源那道 —— 405 会说明守卫排在路由之后')
      assert.equal(foreign.json.error, ERRORS.ORIGIN_NOT_ALLOWED)
      assert.equal(foreign.headers.allow, undefined, '被来源拒掉时不该顺手把 Allow 发给它')
      // 负向对照：**合法来源**同样的打法照旧 405 + `Allow: PUT`（方法判据一个字没动）。
      const ours = await request(port, 'GET', '/home-binding', { headers: { origin: 'http://localhost:5173' } })
      assert.equal(ours.status, 405)
      assert.equal(ours.headers.allow, 'PUT')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('模式二：远程浏览器（Host 必然非 loopback）带钥匙 ⇒ 200（来源守卫不许再卡它）', async () => {
  // 这一条是**真机路径的回归**（2026-09-25）：远程家条目的地址是
  // `https://<家地址>:<config_port>`，Host 必然非 loopback；而来源守卫原本无条件要求
  // loopback ⇒ **带有效家钥匙也一律 403 origin_not_allowed**（页面只会说"只允许从本机访问"）。
  // 旧用例全用默认 Host（127.0.0.1）⇒ 一个都没碰到这条真路径 —— 所以它必须显式改写 Host。
  const root = scratch('remote-host')
  const key = homeKey.generateKey()
  homeKey.writeKeyDigest(root, homeKey.hashKey(key))
  const authorization = { authorization: `Bearer ${key}` }
  try {
    await withServer(
      async ({ port }) => {
        // 真浏览器的形状：跨源直连 + Host 是那台机器的地址。
        const remoteHost = { host: '192.168.1.20:5191' }
        const read = await request(port, 'GET', '/home-config', { headers: { ...remoteHost, ...authorization } })
        assert.equal(read.status, 200, `远程读被来源守卫卡住了：${read.text}`)
        assert.equal(read.json.state, 'absent')
        // 门答 200 ≠ 页面拿得到：跨源那一份还必须带 CORS 头（模式二就是为这个浏览器而设）。
        assert.equal(read.headers['access-control-allow-origin'], '*')

        const write = await request(port, 'PUT', '/home-config', {
          body: body(),
          headers: { ...remoteHost, ...authorization, origin: 'https://192.168.1.20' },
        })
        assert.equal(write.status, 200, `远程写被来源守卫卡住了：${write.text}`)

        // 负向对照：模式二里**钥匙**才是闸 —— 同一套 Host、没有钥匙 ⇒ 401（不是 403）。
        const noKey = await request(port, 'GET', '/home-config', { headers: remoteHost })
        assert.equal(noKey.status, 401)
        assert.equal(noKey.json.error, homeKey.UNAUTHORIZED)
      },
      { homeDir: root },
      { access: { mode: 'remote', homeDir: root } },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 读写与"口上无秘密"
// ---------------------------------------------------------------------------

configCase('GET：一条都没有 ⇒ state=absent（不许把"没配"画成配置好了）', async () => {
  const root = scratch('absent')
  try {
    await withServer(async ({ port }) => {
      const res = await request(port, 'GET', '/home-config')
      assert.equal(res.json.state, 'absent')
      assert.equal(res.json.keyConfigured, false)
      assert.equal(res.json.endpoint, null)
      assert.equal(typeof res.json.readAt, 'string')
      // 三合一的三层各自如实：清单空、绑定空、派生为空。
      assert.equal(res.json.sourcesState, 'absent')
      assert.deepEqual(res.json.entries, [])
      assert.equal(res.json.bindingState, 'absent')
      assert.equal(res.json.bound, null)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('POST：新建一条 ⇒ 清单里多一条 `ms-1`，键名与 Rust 读取器逐字对齐，且试连恰好一次', async () => {
  const root = scratch('create')
  try {
    await withServer(async ({ port, probes }) => {
      const res = await request(port, 'POST', '/home-config', { body: body() })
      assert.equal(res.status, 200, res.text)
      assert.equal(res.json.state, 'configured')
      assert.equal(res.json.keyConfigured, true)
      assert.equal(res.json.endpoint, 'https://api.example.invalid/chat/completions')
      assert.equal(res.json.model, 'some-model')
      assert.equal(res.json.contextWindow, 131072)
      assert.equal(res.json.protocol, null, '不给第五件就按本家那一条走')
      assert.equal(res.json.entryId, 'ms-1', 'id 由门给')
      assert.equal(res.json.creating, true)
      assert.equal(probes.length, 1, '保存后**恰好**试连一次')
      assert.equal(probes[0].endpoint, 'https://api.example.invalid/chat/completions')

      const onDisk = readList(root)
      assert.equal(onDisk.length, 1)
      // ⚠ 这一格守的是"**写者落盘的键集**"（给 Rust 读取器的那一份，snake_case）：四件 ＋
      // id / name ＋ 思考强度 ＋ 视觉。`reasoning_effort` 是 2026-10-08 由 `766204c`（JS 清单那个写者）
      // 加进来的，而这条断言当时没跟上 ⇒ 家脚本套件一直红着一条（2026-10-09 按主人指示修掉）。
      // 真加了第九格、或谁悄悄少了一格，这里当场红。
      // `vision`（2026-10-17）就是"真加了第八格"那一次——它同时是那个病的疫苗：
      // **页面不认识的格，门必须在保存时原样带过来**（见 `upsertEntry` 的注释）。
      assert.deepEqual(
        Object.keys(onDisk[0]).sort(),
        [
          'api_key',
          'context_window',
          'endpoint',
          'id',
          'model',
          'name',
          'reasoning_effort',
          'vision',
        ],
        '条目的键名是给 Rust 读取器的（snake_case，四件 + id / name + 思考强度 + 视觉）',
      )
      assert.equal(onDisk[0].api_key, 'sk-secret-value')
      assert.equal(onDisk[0].id, 'ms-1')
      // **保存 = 用它**（缺省 `bind: true`）：绑定指过来了。
      assert.equal(readBinding(root).source, 'ms-1')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('GET 的三合一：清单 / 绑定 / 派生三层各说各的话（派生是从绑定算的，不是第二份权威）', async () => {
  const root = scratch('triple')
  try {
    await withServer(async ({ port }) => {
      // 两条：ms-1（绑定的）与 ms-2（只存清单，`bind: false`）。
      await request(port, 'POST', '/home-config', { body: body({ name: '主钥匙' }) })
      await request(port, 'POST', '/home-config', {
        body: body({ name: '备用', endpoint: 'https://backup.invalid/v1/chat/completions', model: 'backup-model', apiKey: 'sk-2', bind: false }),
      })
      const res = await request(port, 'GET', '/home-config')
      assert.equal(res.json.sourcesState, 'loaded')
      assert.deepEqual(
        res.json.entries.map((entry) => entry.id),
        ['ms-1', 'ms-2'],
      )
      assert.equal(res.json.entries[0].name, '主钥匙')
      assert.equal(res.json.entries[1].keyConfigured, true)
      assert.equal(res.json.bindingState, 'bound')
      assert.equal(res.json.bound, 'ms-1', '`bind: false` 不该动绑定')
      // 派生字段 = **绑定那一条**的（改绑之后它会跟着变——见下一条用例）。
      assert.equal(res.json.endpoint, 'https://api.example.invalid/chat/completions')
      assert.equal(res.json.model, 'some-model')
      assert.ok(!res.text.includes('sk-2'), '条目的 key 一个字节都不回浏览器')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('换绑：`PUT /home-binding` 之后派生字段跟着变；发空 source = 解绑（清单不动）', async () => {
  const root = scratch('rebind')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      await request(port, 'POST', '/home-config', {
        body: body({ endpoint: 'https://backup.invalid/v1/chat/completions', model: 'backup-model', apiKey: 'sk-2', bind: false }),
      })
      const switched = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-2' } })
      assert.equal(switched.status, 200, switched.text)
      assert.equal(switched.json.bound, 'ms-2')
      assert.equal(switched.json.endpoint, 'https://backup.invalid/v1/chat/completions')
      assert.equal(switched.json.model, 'backup-model')
      // 绑到不在场的条目 ⇒ 404（不静默建一条）。
      const missing = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-9' } })
      assert.equal(missing.status, 404)
      assert.equal(missing.json.error, ERRORS.ENTRY_NOT_FOUND)
      // 空 source = 解绑：派生清空、绑定文件没了，但**清单一条不少**。
      const unbound = await request(port, 'PUT', '/home-binding', { body: { source: '' } })
      assert.equal(unbound.status, 200, unbound.text)
      assert.equal(unbound.json.state, 'absent')
      assert.equal(unbound.json.bindingState, 'absent')
      assert.equal(unbound.json.bound, null)
      assert.equal(unbound.json.endpoint, null)
      assert.equal(unbound.json.entries.length, 2, '解绑不动清单')
      assert.ok(!fs.existsSync(bindingFile(root)))
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('**口上无秘密**：GET 的响应里永远没有 key，只有 keyConfigured 一个布尔', async () => {
  const root = scratch('nosecret')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      const res = await request(port, 'GET', '/home-config')
      assert.ok(!res.text.includes('sk-secret-value'), `响应里漏了 key：${res.text}`)
      assert.equal(res.json.keyConfigured, true)
      assert.equal(res.json.apiKey, undefined)
      // 清单那一层同样只有"配没配"：条目里不许出现 key 的键。
      assert.equal(res.json.entries[0].api_key, undefined)
      assert.equal(res.json.entries[0].apiKey, undefined)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('PUT 不带 id：改的是**当前绑定的那一条**；密钥留空 = 沿用它的那一把', async () => {
  const root = scratch('reuse')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      const res = await request(port, 'PUT', '/home-config', {
        body: body({ apiKey: '', model: 'another-model' }),
      })
      assert.equal(res.status, 200, res.text)
      assert.equal(res.json.model, 'another-model')
      assert.equal(res.json.entryId, 'ms-1')
      assert.equal(res.json.creating, false, '这是改，不是新建')
      const onDisk = readList(root)
      assert.equal(onDisk.length, 1, '改一条不该多出一条')
      assert.equal(onDisk[0].api_key, 'sk-secret-value', '留空沿用，不是清空')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('POST 新建时**必须**给 key（新建没有可沿用的东西）', async () => {
  const root = scratch('firstkey')
  try {
    await withServer(async ({ port }) => {
      const res = await request(port, 'POST', '/home-config', { body: body({ apiKey: '' }) })
      assert.equal(res.status, 400)
      assert.equal(res.json.error, ERRORS.KEY_REQUIRED)
      assert.ok(!fs.existsSync(listFile(root)), '被拒的保存不该落盘')
      assert.ok(!fs.existsSync(bindingFile(root)), '被拒的保存不该动绑定')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('POST **不接受**调用方指定的 id（新建的 id 由门给，不是覆盖）', async () => {
  const root = scratch('post-with-id')
  try {
    await withServer(async ({ port }) => {
      const res = await request(port, 'POST', '/home-config', { body: body({ id: 'ms-1' }) })
      assert.equal(res.status, 400)
      assert.equal(res.json.error, ERRORS.BAD_REQUEST)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('PUT 点名 id：改那一条（不在场就 404，绝不悄悄新建）；`bind` 形状不对 ⇒ 明确拒绝', async () => {
  const root = scratch('put-by-id')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body({ name: '第一条' }) })
      await request(port, 'POST', '/home-config', {
        body: body({ name: '第二条', model: 'second-model', apiKey: 'sk-2', bind: false }),
      })
      // 点 ms-2 的名字改它：绑定仍指向 ms-1（`bind: false`）。
      const edited = await request(port, 'PUT', '/home-config', {
        body: body({ id: 'ms-2', name: '改过名', apiKey: '', model: 'second-model-v2', bind: false }),
      })
      assert.equal(edited.status, 200, edited.text)
      assert.equal(edited.json.bound, 'ms-1', '改备用条不该换绑')
      assert.deepEqual(
        readList(root).map((entry) => `${entry.id}:${entry.name}:${entry.api_key}`),
        ['ms-1:第一条:sk-secret-value', 'ms-2:改过名:sk-2'],
      )
      // 不在场的 id ⇒ 404（不新建）。
      const missing = await request(port, 'PUT', '/home-config', { body: body({ id: 'ms-9' }) })
      assert.equal(missing.status, 404)
      assert.equal(missing.json.error, ERRORS.ENTRY_NOT_FOUND)
      // 形状不对的 id ⇒ invalid_id（与"不在场"是两件事）。
      const badShape = await request(port, 'PUT', '/home-config', { body: body({ id: 'sec-1' }) })
      assert.equal(badShape.status, 400)
      assert.equal(badShape.json.error, ERRORS.INVALID_ID)
      // `bind` 只认布尔（不把字符串 "false" 当真）。
      const badBind = await request(port, 'POST', '/home-config', {
        body: body({ apiKey: 'sk-3', bind: 'false' }),
      })
      assert.equal(badBind.status, 400)
      assert.equal(badBind.json.error, ERRORS.INVALID_BIND)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('PUT 的词表：四件各错一次 ⇒ 各自的稳定错误串（封闭表）', async () => {
  const root = scratch('words')
  try {
    await withServer(async ({ port }) => {
      for (const [expected, overrides] of [
        [ERRORS.INVALID_ENDPOINT, { endpoint: 'not-a-url' }],
        [ERRORS.INVALID_ENDPOINT, { endpoint: 'ftp://example.invalid' }],
        [ERRORS.INVALID_MODEL, { model: '  ' }],
        [ERRORS.INVALID_WINDOW, { contextWindow: 0 }],
        [ERRORS.INVALID_WINDOW, { contextWindow: 'big' }],
        // ⚠ 空窗口**不再是** `invalid_window`（TASK_77 回炉③起它是"自动获取"那条路，
        // 走不通时回 `window_unavailable`——见下面那两条）。这一格测的是**填了垃圾**。
        [ERRORS.INVALID_WINDOW, { contextWindow: '1.5' }],
        [ERRORS.INVALID_WINDOW, { contextWindow: -5 }],
        [ERRORS.INVALID_PROTOCOL, { protocol: 'grpc' }],
      ]) {
        const res = await request(port, 'PUT', '/home-config', { body: body(overrides) })
        assert.equal(res.status, 400, `${JSON.stringify(overrides)} ⇒ ${res.text}`)
        assert.equal(res.json.error, expected, JSON.stringify(overrides))
      }
      // 体不是 JSON ⇒ bad_request（不是 500）。
      const malformed = await new Promise((resolve, reject) => {
        const req = http.request(
          { host: '127.0.0.1', port, method: 'PUT', path: '/home-config', headers: { 'content-type': 'application/json' } },
          (res) => {
            let text = ''
            res.on('data', (chunk) => {
              text += chunk
            })
            res.on('end', () => resolve({ status: res.statusCode, text }))
          },
        )
        req.on('error', reject)
        req.write('{ 这不是 JSON')
        req.end()
      })
      assert.equal(malformed.status, 400)
      assert.equal(JSON.parse(malformed.text).error, ERRORS.BAD_REQUEST)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('试连失败**不拦保存**：如实回失败读数，条目照旧落盘', async () => {
  const root = scratch('probefail')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', { body: body() })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.state, 'configured', '试连失败不该把配置说成没配上')
        assert.equal(res.json.probe.ok, false)
        assert.equal(res.json.probe.status, 401, 'HTTP 状态码那一格要如实回')
        assert.equal(res.json.probe.reason, null)
        assert.ok(fs.existsSync(listFile(root)), '试连失败也照旧保存')
      },
      {
        homeDir: root,
        probe: async (endpoint) => ({
          url: modelsUrl(endpoint),
          ok: false,
          status: 401,
          reason: null,
          ...NO_CATALOG,
        }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('试连：连不上与超时分开说（status 为 null，reason 点名是哪一种；窗口两格为 null）', async () => {
  const refused = await probeModels('https://127.0.0.1:1/chat/completions', 'sk-x', 'some-model', {
    timeoutMs: 2000,
  })
  assert.equal(refused.ok, false)
  assert.equal(refused.status, null)
  assert.equal(refused.reason, 'unreachable')
  assert.ok(refused.url.endsWith('/models'), refused.url)
  assert.deepEqual(
    { contextWindow: refused.contextWindow, windowField: refused.windowField },
    NO_WINDOW,
    '连不上时窗口两格必须是 null（不是 undefined：客户端那份守卫要求它们在场）',
  )
})

configCase('DELETE 不带 id：**解除绑定**（撤销式 ⇒ 查无也成功，幂等读数 absent），清单一条不少', async () => {
  const root = scratch('unbind')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      const first = await request(port, 'DELETE', '/home-config')
      assert.equal(first.status, 200)
      assert.equal(first.json.state, 'absent')
      assert.equal(first.json.keyConfigured, false)
      assert.equal(first.json.bindingState, 'absent')
      assert.ok(!fs.existsSync(bindingFile(root)), '绑定摘掉了')
      assert.equal(readList(root).length, 1, '**条目还在**：撤回一条路不等于销毁那份清单')
      // 重删无害：本机门、无公共事实、last-write-wins（不建幂等键机制）。
      const second = await request(port, 'DELETE', '/home-config')
      assert.equal(second.status, 200)
      assert.equal(second.json.state, 'absent')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('DELETE 带 id：删清单条目；**被绑定挡住就拒**（409 + 说明被谁绑着）', async () => {
  const root = scratch('delete-entry')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body({ name: '在用' }) })
      await request(port, 'POST', '/home-config', {
        body: body({ model: 'second-model', apiKey: 'sk-2', bind: false }),
      })
      // ms-1 正被绑着 ⇒ 拒（先换绑再删）。
      const blocked = await request(port, 'DELETE', '/home-config?id=ms-1')
      assert.equal(blocked.status, 409)
      assert.equal(blocked.json.error, ERRORS.ENTRY_BOUND)
      assert.equal(blocked.json.boundTo, 'ms-1')
      assert.equal(readList(root).length, 2, '被拒的删除不该动清单')
      // 换绑到 ms-2 之后就能删 ms-1 了。
      await request(port, 'PUT', '/home-binding', { body: { source: 'ms-2' } })
      const removed = await request(port, 'DELETE', '/home-config?id=ms-1')
      assert.equal(removed.status, 200, removed.text)
      assert.deepEqual(
        readList(root).map((entry) => entry.id),
        ['ms-2'],
      )
      // 删一条不在场的 ⇒ 404（与"被挡住"分开说）。
      const missing = await request(port, 'DELETE', '/home-config?id=ms-9')
      assert.equal(missing.status, 404)
      assert.equal(missing.json.error, ERRORS.ENTRY_NOT_FOUND)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

/**
 * ★ **两支住户各绑一条：删哪一条都被拒**（2026-10-23：模块层从前只看第一支）。
 *
 * 走**真实的 HTTP 路由**（不是直接调模块）：判据是"门传给模块的是不是**每一支**的
 * 环境根"。用一份固定的住户表注入（`ctx.residents`），第二支故意是**唯一**绑着 `ms-2` 的
 * 那一支 —— 只看第一支的实现会在这里把 `ms-2` 删掉，而那一支下次开机就报"模型源已被删除"。
 */
configCase('★ DELETE：两支住户各绑一条 ⇒ 删**任何一条**都被拒（问的是每一支）', async () => {
  const root = scratch('delete-across-residents')
  const first = path.join(root, 'agents', 'a-1')
  const second = path.join(root, 'agents', 'b-2')
  try {
    fs.mkdirSync(first, { recursive: true })
    fs.mkdirSync(second, { recursive: true })
    fs.writeFileSync(path.join(first, 'identity.json'), '{"agent":"a-1"}')
    fs.writeFileSync(path.join(second, 'identity.json'), '{"agent":"b-2"}')
    modelSources.writeList(
      root,
      [
        { id: 'ms-1', name: '主钥匙', endpoint: 'https://a.invalid/v1/chat/completions', api_key: 'k1', model: 'm1', context_window: 8192 },
        { id: 'ms-2', name: '备用', endpoint: 'https://b.invalid/v1/chat/completions', api_key: 'k2', model: 'm2', context_window: 4096 },
        { id: 'ms-3', name: '没人用', endpoint: 'https://c.invalid/v1/chat/completions', api_key: 'k3', model: 'm3', context_window: 2048 },
      ],
      4,
    )
    modelSources.writeBinding(first, 'ms-1')
    modelSources.writeBinding(second, 'ms-2')
    await withServer(
      async ({ port }) => {
        // 第一支绑着 ms-1 ⇒ 拒（老行为，照旧）。
        const one = await request(port, 'DELETE', '/home-config?id=ms-1')
        assert.equal(one.status, 409, one.text)
        assert.equal(one.json.error, ERRORS.ENTRY_BOUND)
        assert.equal(one.json.boundTo, 'ms-1')
        // **只有第二支**绑着 ms-2 ⇒ 也拒（这一条从前会漏）。
        const two = await request(port, 'DELETE', '/home-config?id=ms-2')
        assert.equal(two.status, 409, two.text)
        assert.equal(two.json.error, ERRORS.ENTRY_BOUND)
        assert.equal(two.json.boundTo, 'ms-2')
        assert.deepEqual(
          readList(root).map((entry) => entry.id),
          ['ms-1', 'ms-2', 'ms-3'],
          '被拒的两次一个字节都不动',
        )
        // 没人绑的那条 ⇒ 删得掉（挡板不是"一律拒"）。
        const free = await request(port, 'DELETE', '/home-config?id=ms-3')
        assert.equal(free.status, 200, free.text)
        assert.deepEqual(
          readList(root).map((entry) => entry.id),
          ['ms-1', 'ms-2'],
        )
      },
      {
        homeDir: root,
        residents: () => [
          { agentId: 'a-1', envDir: first },
          { agentId: 'b-2', envDir: second },
        ],
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('原子写：写完之后两个目录里都不留下临时文件（Agent 永远读不到半行）', async () => {
  const root = scratch('atomic')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      const envNames = fs.readdirSync(path.join(root, 'env'))
      assert.deepEqual(envNames, ['model_binding.json'], `环境根里只该有绑定：${envNames.join(', ')}`)
      const homeNames = fs.readdirSync(root).sort()
      assert.deepEqual(
        homeNames,
        ['env', 'logs', 'model_sources.json'],
        `家根里只该有清单、env 与见证账本（logs）：${homeNames.join(', ')}`,
      )
      // ★ 见证账本也**只追加、不留临时件**（2026-10-18）：`POST` 写了清单**并顺手绑了**这一支
      //   ⇒ 正好两条，各记各的那一份（顺序就是门写它们的顺序）。
      const logs = fs.readdirSync(path.join(root, 'logs')).sort()
      assert.deepEqual(logs, ['model-witness.ndjson'], `见证那一层只该有账本：${logs.join(', ')}`)
      assert.deepEqual(
        modelWitness.readRecords(root).map((record) => record.file),
        ['model_sources.json', 'env/model_binding.json'],
        '两笔写各记一条，**按份**记',
      )
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **见证**（2026-10-18）：这道门改过那两份文件就记一条，读数里**两档分开**报。
//
// 症状（`agent/README.md` §4 那一格）：这一格从前**没有见证** —— 装配横幅报的是"这一拍从哪儿
// 读的"，不是"谁改的"。判据四条，与壳那一侧同一份（`app/src-tauri/src/home_doors.rs` 的
// `the_door_leaves_a_witness_for_every_file_it_writes`）：
// ① **按份记**（清单 / 每一支的绑定各自一条）；② **"清"也是一笔写**（不记它，页面上的
// "解除指定"会读成"别处有人动过它"）；③ **两档分开**：门改的 ⇒ `same` **且有时刻**；
// 门外那只手改的（她走 terminal）⇒ `changed/content`，而**是谁这条路上答不出来**；
// ④ **判不了 ≠ 没改过**：从没被门碰过 ⇒ `no-witness`。
configCase('★ 见证：门写过的两份各记一条；门外那只手改的 ⇒ changed（两档分开）', async () => {
  const root = scratch('witness')
  const envDir = path.join(root, 'agents', 'a-1')
  try {
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(path.join(envDir, 'identity.json'), '{"agent":"a-1"}')
    // ⚠ 这两笔是**直接写文件**（不是门写的）⇒ 见证那一层此刻应当是 `no-witness`。
    modelSources.writeBinding(envDir, 'ms-1')
    modelSources.writeList(root, [
      { id: 'ms-1', name: '主钥匙', endpoint: 'https://a.invalid/v1/chat/completions', api_key: 'k1', model: 'm1', context_window: 8192 },
      { id: 'ms-2', name: '备用', endpoint: 'https://b.invalid/v1/chat/completions', api_key: 'k2', model: 'm2', context_window: 4096 },
    ])
    const bindingPath = modelSources.bindingPath(envDir)
    await withServer(
      async ({ port }) => {
        const view = async () => (await request(port, 'GET', '/home-config')).json
        // ④ 从没被门碰过 ⇒ **判不了**（不是"没改过"），且一个字节都没写。
        const before = await view()
        assert.equal(before.listWitness.state, 'no-witness')
        assert.equal(before.listWitness.atMs, null)
        assert.equal(before.boundPerResident[0].witness.state, 'no-witness')
        assert.equal(fs.existsSync(modelWitness.witnessPath(root)), false, '门还没写过 ⇒ 账本不在')

        // ① 门写一支的绑定 ⇒ 只记那一份（清单没动，照旧判不了）
        const bound = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-2', agentId: 'a-1' } })
        assert.equal(bound.status, 200, bound.text)
        const after = await view()
        assert.equal(after.boundPerResident[0].witness.state, 'same')
        assert.ok(Number.isInteger(after.boundPerResident[0].witness.atMs), '门改的**有时刻**')
        assert.equal(after.listWitness.state, 'no-witness', '没写的那一份照旧判不了')
        assert.deepEqual(
          modelWitness.readRecords(root).map((record) => record.file),
          ['agents/a-1/model_binding.json'],
          '一份文件一条，**按份**记',
        )
        assert.equal(after.witnessChainBreakAt, null, '链是完整的')

        // ③ 门外那只手改的（她走 terminal）⇒ `changed/content`，而"是谁"答不出来
        fs.writeFileSync(bindingPath, '{\n  "source": "ms-1"\n}\n')
        const touched = await view()
        assert.equal(touched.boundPerResident[0].witness.state, 'changed')
        assert.equal(touched.boundPerResident[0].witness.detail, 'content')
        assert.ok(
          Number.isInteger(touched.boundPerResident[0].witness.atMs),
          '知道的只是**门最后写于何时**',
        )

        // ① 门写清单 ⇒ 清单那一份也记一条（`bind:false`：只动清单，不碰绑定）
        const saved = await request(port, 'PUT', '/home-config', {
          body: {
            id: 'ms-1',
            endpoint: 'https://a.invalid/v1/chat/completions',
            api_key: 'k1',
            model: 'm1',
            contextWindow: 8192,
            bind: false,
          },
        })
        assert.equal(saved.status, 200, saved.text)
        const written = await view()
        assert.equal(written.listWitness.state, 'same')
        assert.ok(Number.isInteger(written.listWitness.atMs))
        assert.equal(modelWitness.readRecords(root).length, 2, '两份文件两条（**按份**，不合并）')

        // ② "清"也是一笔写：门摘的 ⇒ 读数是 `same`，而**不是**"别处有人动过它"
        const cleared = await request(port, 'DELETE', '/home-config')
        assert.equal(cleared.status, 200, cleared.text)
        assert.equal(fs.existsSync(bindingPath), false)
        const gone = await view()
        assert.equal(gone.boundPerResident[0].witness.state, 'same')
        assert.equal(gone.boundPerResident[0].witness.detail, null)
        const last = modelWitness.readRecords(root).pop()
        assert.equal(last.file, 'agents/a-1/model_binding.json')
        assert.equal(last.sha256, null, '那一刻它不在 —— 摘要那一格是 null')
      },
      { homeDir: root, residents: () => [{ agentId: 'a-1', envDir }] },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **半途失败也不留"改了却没见证"的那一份**（2026-10-18）：**一支写成就记一条** ——
// 不许等整圈写完再记：那一圈半路抛了，前面**真被改过**的那几份就会读成"别处有人动过它"
// （把第一档报成第二档，而这一层存在的全部理由就是不许把这两档混成一件）。
configCase('★ 半绑失败：先写成的那一份**有见证**（读成"页面改的"，不是"有人动过它"）', async () => {
  const root = scratch('witness-half')
  const first = path.join(root, 'agents', 'a-1')
  // 第二支的"环境根"是一个**文件** ⇒ 写到它必失败（这就是"半绑"：前一支已经写成了）。
  const second = path.join(root, 'agents', 'a-2')
  try {
    fs.mkdirSync(first, { recursive: true })
    fs.writeFileSync(path.join(first, 'identity.json'), '{"agent":"a-1"}')
    fs.writeFileSync(second, 'not a directory')
    modelSources.writeList(root, [
      { id: 'ms-1', name: '主钥匙', endpoint: 'https://a.invalid/v1/chat/completions', api_key: 'k1', model: 'm1', context_window: 8192 },
    ])
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-1' } })
        assert.equal(res.status, 500, `半绑必须**如实失败**（不许静默）：${res.text}`)
        const view = (await request(port, 'GET', '/home-config')).json
        const row = view.boundPerResident.find((each) => each.agentId === 'a-1')
        assert.equal(row.witness.state, 'same', '先写成的那一份有见证 —— 不是"别处有人动过它"')
        assert.deepEqual(
          modelWitness.readRecords(root).map((record) => record.file),
          ['agents/a-1/model_binding.json'],
          '只记真写成的那一份（写失败的那一支没有见证，照旧"判不了"）',
        )
        const failed = view.boundPerResident.find((each) => each.agentId === 'a-2')
        assert.equal(failed.witness.state, 'no-witness')
      },
      {
        homeDir: root,
        residents: () => [
          { agentId: 'a-1', envDir: first },
          { agentId: 'a-2', envDir: second },
        ],
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 迁移与两个文件的关系（刀2b：清单归机器，绑定归 Agent）
// ---------------------------------------------------------------------------

configCase('迁移：老的四件套 ⇒ 清单第一条 + 绑定指向它 + 老文件**改名留档**；重跑幂等', () => {
  const root = scratch('migrate')
  try {
    writeLegacy(
      root,
      JSON.stringify({
        endpoint: 'https://legacy.invalid/v1/chat/completions',
        api_key: 'sk-legacy',
        model: 'legacy-model',
        context_window: 8192,
      }),
    )
    const first = modelSources.migrateIfNeeded(root, path.join(root, 'env'))
    assert.equal(first.action, 'migrated', JSON.stringify(first))
    assert.equal(first.entry.id, 'ms-1')
    assert.deepEqual(
      readList(root).map(
        (entry) => `${entry.id}:${entry.endpoint}:${entry.model}:${entry.context_window}`,
      ),
      ['ms-1:https://legacy.invalid/v1/chat/completions:legacy-model:8192'],
      '四件原样迁过来，一件不少',
    )
    assert.equal(readList(root)[0].api_key, 'sk-legacy')
    assert.equal(readBinding(root).source, 'ms-1')
    assert.ok(!fs.existsSync(path.join(root, 'env', 'model_source.json')), '老文件不再是权威')
    assert.ok(fs.existsSync(path.join(root, 'env', 'model_source.json.migrated')), '改名留档')

    // **幂等**：再跑一次什么都不做，条目数不变、绑定不动、留档件不叠加第二份。
    const again = modelSources.migrateIfNeeded(root, path.join(root, 'env'))
    assert.equal(again.action, 'skipped', JSON.stringify(again))
    assert.equal(readList(root).length, 1)
    assert.equal(readBinding(root).source, 'ms-1')
    assert.equal(fs.readdirSync(path.join(root, 'env')).length, 2, '绑定 + 一份留档，就这些')
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('迁移：老文件与绑定**同时在场**（双遗迹）⇒ 如实报 conflict，**不自动选边**', () => {
  const root = scratch('double-relic')
  try {
    writeLegacy(
      root,
      JSON.stringify({ endpoint: 'https://legacy.invalid', api_key: 'k', model: 'm', context_window: 8192 }),
    )
    modelSources.writeBinding(path.join(root, 'env'), 'ms-1')
    const got = modelSources.migrateIfNeeded(root, path.join(root, 'env'))
    assert.equal(got.action, 'conflict', JSON.stringify(got))
    // 两个文件都还在，清单**没被写**（不选边 = 一个字节都不动）。
    assert.ok(fs.existsSync(path.join(root, 'env', 'model_source.json')))
    assert.ok(fs.existsSync(path.join(root, 'env', 'model_binding.json')))
    assert.ok(!fs.existsSync(listFile(root)), '双遗迹时不该凭空造一份清单')
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 读侧的形状（与 Rust 读取器同一条口径）
// ---------------------------------------------------------------------------

configCase('读侧：**悬空绑定**（绑的那条没了）⇒ unreadable + `binding_dangling`，不回落成"没配"', async () => {
  const root = scratch('dangling')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', '/home-config', { body: body() })
      // 直接把清单写成空（模拟"那条被别的路删掉了"）。
      modelSources.writeList(root, [])
      const res = await request(port, 'GET', '/home-config')
      assert.equal(res.json.bindingState, 'bound')
      assert.equal(res.json.bound, 'ms-1')
      assert.equal(res.json.state, 'unreadable', '悬空绑定是"读不出来"，不是"没配"')
      assert.equal(res.json.reason, 'binding_dangling')
      assert.equal(res.json.endpoint, null)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('读侧：清单坏了 / 条目全不认识 / 绑定坏了 ⇒ 三种都如实说，不混成"没配"', () => {
  const root = scratch('unreadable')
  try {
    // ① 清单不是 JSON。
    fs.writeFileSync(listFile(root), '{ 这不是 JSON', 'utf8')
    const broken = modelSources.readList(root)
    assert.equal(broken.state, 'unreadable')
    assert.equal(broken.reason, 'not_json')

    // ② 清单是数组、但一条都不认得（缺件）⇒ `no_valid_entry`（不假装"空清单"）。
    fs.writeFileSync(
      listFile(root),
      JSON.stringify([{ id: 'ms-1', endpoint: 'https://x.invalid', api_key: 'sk-leak' }]),
      'utf8',
    )
    const noEntry = modelSources.readList(root)
    assert.equal(noEntry.state, 'unreadable')
    assert.equal(noEntry.reason, 'no_valid_entry', '缺件与"不是 JSON"是两种处境')

    // ③ 绑定坏了 ⇒ 也是 unreadable（不当作"没绑"）。
    const envDir = path.join(root, 'env')
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(bindingFile(root), '{}', 'utf8')
    assert.equal(modelSources.readBinding(envDir).state, 'unreadable')

    // ④ **负向对照**：读不出来的读数里一个 key 字节都没有（那两格只有状态与原因）。
    assert.ok(!JSON.stringify({ state: noEntry.state, reason: noEntry.reason }).includes('sk-leak'))
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('模型目录（刀2b）：保序去重、只认那三个名字来源、垃圾条目跳过', () => {
  assert.deepEqual(
    modelsFromBody({
      data: [
        { id: 'b-model' },
        { name: 'a-model' },
        { model: 'c-model' },
        { id: 'b-model' },
        { id: '   ' },
        null,
        'not-an-entry',
        {},
        { id: 'd-model', context_length: 4096 },
      ],
    }),
    ['b-model', 'a-model', 'c-model', 'd-model'],
    '目录顺序照原样；重名只留一个；没有名字的条目跳过',
  )
  // 名字**逐字原样**（不去空格之外的东西；大小写不动）。
  assert.deepEqual(modelsFromBody({ models: [{ id: '  spaced  ' }] }), ['spaced'])
  assert.deepEqual(modelsFromBody([{ id: 'only-one' }]), ['only-one'])
  // 不认识的形状 / 空 / 没读过 ⇒ 空数组（**不是错误**）。
  for (const body of [null, {}, { list: [{ id: 'x' }] }, { data: [] }]) {
    assert.deepEqual(modelsFromBody(body), [], JSON.stringify(body))
  }
  // 封顶：目录再长也只带回前 MODEL_LIST_LIMIT 个（它是读数，不是要落盘的数据）。
  const many = Array.from({ length: MODEL_LIST_LIMIT + 50 }, (_, i) => ({ id: `m-${i}` }))
  assert.equal(modelsFromBody({ data: many }).length, MODEL_LIST_LIMIT)
  assert.equal(modelsFromBody({ data: many })[0], 'm-0', '保序：从第一个开始截')
})

configCase('模型目录与窗口**同一份体**里取（体只读一次：读两遍会读空）', async () => {
  const provider = http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(
      JSON.stringify({
        data: [
          { id: 'our-model', context_length: 65536 },
          { id: 'another-model', context_length: 8192 },
        ],
      }),
    )
  })
  await new Promise((resolve) => provider.listen(0, '127.0.0.1', resolve))
  const providerPort = provider.address().port
  try {
    const got = await probeModels(
      `http://127.0.0.1:${providerPort}/v1/chat/completions`,
      'sk-probe',
      'our-model',
      { timeoutMs: 3000 },
    )
    assert.equal(got.contextWindow, 65536)
    assert.deepEqual(
      got.models,
      ['our-model', 'another-model'],
      '窗口与目录从同一份回执里取（两次 text() 在流上会读空）',
    )
  } finally {
    await new Promise((resolve) => provider.close(resolve))
  }
})

configCase('试连地址：**端点去掉线协议那一段**再拼 /models（端点是完整地址）', () => {
  assert.equal(modelsUrl('https://api.example.invalid/chat/completions'), 'https://api.example.invalid/models')
  assert.equal(modelsUrl('https://api.example.invalid/v1/chat/completions'), 'https://api.example.invalid/v1/models')
  assert.equal(modelsUrl('https://api.example.invalid/v1/responses'), 'https://api.example.invalid/v1/models')
  assert.equal(modelsUrl('https://api.example.invalid/v1/'), 'https://api.example.invalid/v1/models')
  assert.equal(modelsUrl('https://api.example.invalid'), 'https://api.example.invalid/models')
})

configCase('口的进程环境：白名单 ＋ CELIRA_HOME_DIR，家的凭据一个字节都不进去', () => {
  const env = serveEnv(
    {
      Path: 'x',
      SystemRoot: 'y',
      TEMP: 'z',
      DEEPSEEK_API_KEY: 'sk-secret-value',
      CELIRA_IM_BACKEND_AUTH: 'clr_bc_secret',
      CELIRA_MODEL_API_KEY: 'sk-other',
      HUMAN_TOKEN: 'tok',
    },
    'D:/celira-home',
  )
  assert.equal(env.CELIRA_HOME_DIR, 'D:/celira-home')
  for (const key of ['DEEPSEEK_API_KEY', 'CELIRA_IM_BACKEND_AUTH', 'CELIRA_MODEL_API_KEY', 'HUMAN_TOKEN']) {
    assert.ok(!(key in env), `口的进程环境里不该有 ${key}`)
  }
})

// ---------------------------------------------------------------------------
// 窗口自动探测（TASK_77 回炉③：窗口不是配置，是体检——系统自己查）
// ---------------------------------------------------------------------------

configCase('窗口提取：按 id 命中我们声明的那个模型，读它那一格并**点名字段**', () => {
  const got = windowFromModels(
    {
      object: 'list',
      data: [
        { id: 'other-model', context_length: 4096 },
        { id: 'our-model', context_length: 131072 },
      ],
    },
    'our-model',
  )
  assert.deepEqual(got, { contextWindow: 131072, windowField: 'context_length' })
})

configCase('窗口提取：认哪些字段是**一张有序的表**（先命中的赢，并点名它）', () => {
  for (const field of WINDOW_FIELDS) {
    const entry = { id: 'm' }
    if (field.includes('.')) {
      const [head, tail] = field.split('.')
      entry[head] = { [tail]: 2048 }
    } else {
      entry[field] = 2048
    }
    const got = windowFromModels({ data: [entry] }, 'm')
    assert.equal(got.contextWindow, 2048, field)
    assert.equal(got.windowField, field, field)
  }
  // 两条都在场 ⇒ 表里靠前的那条赢（顺序是判据的一部分）。
  const both = windowFromModels({ data: [{ id: 'm', context_length: 1, context_window: 2 }] }, 'm')
  assert.equal(both.windowField, WINDOW_FIELDS[0])
})

configCase('窗口提取：**宁可少读也不猜**的三条边界', () => {
  // ① 模型不在目录里 ⇒ 不回落"取第一条"
  assert.deepEqual(windowFromModels({ data: [{ id: 'x', context_length: 4096 }] }, 'm'), NO_WINDOW)
  // ② 在目录里、但没有我们认得的字段 ⇒ 没读到
  assert.deepEqual(windowFromModels({ data: [{ id: 'm', owned_by: 'x' }] }, 'm'), NO_WINDOW)
  // ③ 那一格是垃圾（0 / 负 / 非数字串 / 小数 / null / 对象）⇒ 没读到
  for (const bad of [0, -1, 'big', 12.5, null, {}]) {
    assert.deepEqual(
      windowFromModels({ data: [{ id: 'm', context_length: bad }] }, 'm'),
      NO_WINDOW,
      String(bad),
    )
  }
  // 正对照：**恰好是数字的字符串**无歧义，认。
  assert.equal(windowFromModels({ data: [{ id: 'm', context_length: '8192' }] }, 'm').contextWindow, 8192)
})

configCase('窗口提取：只认三种目录形状（`data` / `models` / 回执本身是数组），别处不认识', () => {
  assert.equal(windowFromModels([{ id: 'm', context_window: 1024 }], 'm').contextWindow, 1024)
  assert.equal(windowFromModels({ models: [{ name: 'm', context_window: 1024 }] }, 'm').contextWindow, 1024)
  assert.equal(windowFromModels({ list: [{ id: 'm', context_window: 1024 }] }, 'm').contextWindow, null)
  assert.equal(windowFromModels(null, 'm').contextWindow, null)
  assert.equal(windowFromModels({ data: [] }, 'm').contextWindow, null)
})

configCase('真发一次试连（假供应商）：端点尾巴摘掉 + 窗口读数一起带回来', async () => {
  const seen = []
  const provider = http.createServer((req, res) => {
    seen.push({ url: req.url, auth: req.headers.authorization })
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ data: [{ id: 'our-model', context_length: 65536 }] }))
  })
  await new Promise((resolve) => provider.listen(0, '127.0.0.1', resolve))
  const providerPort = provider.address().port
  try {
    const got = await probeModels(
      `http://127.0.0.1:${providerPort}/v1/chat/completions`,
      'sk-probe',
      'our-model',
      { timeoutMs: 3000 },
    )
    assert.equal(got.ok, true)
    assert.equal(got.status, 200)
    // ⚠ 端点收的是**完整线协议地址** ⇒ 探的必须是摘掉尾巴之后的 `/v1/models`。
    assert.equal(seen[0].url, '/v1/models')
    assert.equal(seen[0].auth, 'Bearer sk-probe')
    assert.equal(got.contextWindow, 65536)
    assert.equal(got.windowField, 'context_length')
  } finally {
    await new Promise((resolve) => provider.close(resolve))
  }
})

configCase('窗口留空：门顺手问出来 ⇒ 落盘的是探到的值，读数说 `probed`', async () => {
  const root = scratch('window-probed')
  const calls = []
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', { body: body({ contextWindow: '' }) })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.windowSource, 'probed')
        assert.equal(res.json.contextWindow, 131072)
        assert.equal(calls.length, 1, '留空那条路只探一次（不重复打第二枪）')
        assert.equal(calls[0].model, 'some-model', '探针要知道问的是哪个模型')
        assert.equal(readList(root)[0].context_window, 131072, '探到的窗口要进清单——Agent 读的就是它')
      },
      {
        homeDir: root,
        probe: async (endpoint, credential, model) => {
          calls.push({ endpoint, credential, model })
          return {
            url: modelsUrl(endpoint),
            ok: true,
            status: 200,
            reason: null,
            contextWindow: 131072,
            windowField: 'context_length',
          }
        },
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('窗口留空 + 探不到 ⇒ `window_unavailable`，**一条都不落盘**（没窗口的配置 Agent 读不了）', async () => {
  const root = scratch('window-unavailable')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', { body: body({ contextWindow: '' }) })
        assert.equal(res.status, 400, res.text)
        assert.equal(res.json.error, ERRORS.WINDOW_UNAVAILABLE)
        // 拒绝时也要把**为什么没问到**的读数交回去（否则人只看到"要你手填"）。
        assert.equal(res.json.probe.ok, false)
        assert.equal(res.json.probe.status, null)
        assert.equal(res.json.probe.reason, 'unreachable')
        assert.ok(!fs.existsSync(listFile(root)), '被拒的保存绝不落盘')
        assert.ok(!fs.existsSync(bindingFile(root)), '被拒的保存也不动绑定')
      },
      {
        homeDir: root,
        probe: async (endpoint) => ({
          url: modelsUrl(endpoint),
          ok: false,
          status: null,
          reason: 'unreachable',
          ...NO_CATALOG,
        }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('给了窗口 ⇒ 以人填的为准（`given`）；探到别的值也不覆盖，试连失败照样保存', async () => {
  const root = scratch('window-given')
  const calls = []
  try {
    await withServer(
      async ({ port }) => {
        // 探针故意回一个**不一样**的值 + 失败读数：两条都断言（"人填的赢"与"失败不拦保存"）。
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: 4096 }),
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.windowSource, 'given')
        assert.equal(res.json.contextWindow, 4096, '人填的赢')
        assert.equal(calls.length, 1)
        assert.equal(readList(root)[0].context_window, 4096, '清单里也是人填的那个')
      },
      {
        homeDir: root,
        probe: async (endpoint) => {
          calls.push({ endpoint })
          return {
            url: modelsUrl(endpoint),
            ok: false,
            status: 401,
            reason: null,
            contextWindow: 999999,
            windowField: 'context_length',
          }
        },
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 只读探测（`POST /home-config/probe`，2026-09-23 预设供应商层）
//
// 这一段的判据只有一件事：**探测只读**。所以每条用例都盯着"文件动没动"，
// 而不是盯着"响应对不对" —— 后者是表面，前者是这一条路径存在的全部意义
// （选供应商、贴 key、拉模型目录，全都是**还没决定要存**的时候做的事）。
// ---------------------------------------------------------------------------

/** 一份文件的指纹（不存在 ⇒ null）：判"一个字节都没动"要靠它。 */
function fingerprint(file) {
  return fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null
}

/** 清单条目（文件不在 ⇒ 空数组：探测那几条用例里"文件本来就不在"是常事）。 */
function listEntries(homeDir) {
  return fs.existsSync(listFile(homeDir)) ? readList(homeDir) : []
}

/** 探测请求体。⚠ 键名是 `api_key`（**门侧那份形状**），与保存那条的 `apiKey` 有意不同 —— 见下面那条用例。 */
function probeBody(overrides = {}) {
  return {
    endpoint: 'https://api.example.invalid/v1/chat/completions',
    api_key: 'sk-probe-canary-2f9c',
    ...overrides,
  }
}

/**
 * **黑盒**那一档要的"那一家"：一个只回 401 的假上游（真 socket、真 HTTP）。
 *
 * 为什么要有它：注入替身的用例证明的是**接线**（打了几枪、URI 是什么），证明不了
 * **真 fetch 走完全程之后**的读数形状（`status` / `reason` 两格怎么分工）。
 * 指向一个假上游 = 真网络栈 + 确定性结果 + **不碰外网**。
 */
async function withUpstream401(fn) {
  const upstream = http.createServer((req, res) => {
    res.writeHead(401, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ error: { message: 'invalid api key' } }))
  })
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve))
  const port = upstream.address().port
  try {
    await fn({ port, endpoint: `http://127.0.0.1:${port}/v1/chat/completions` })
  } finally {
    await new Promise((resolve) => upstream.close(resolve))
  }
}

/** 刚放掉的一个端口（几乎必然没人监听）——让"连不上"这件事**确定性**发生。 */
async function closedPort() {
  const probe = http.createServer(() => {})
  await new Promise((resolve) => probe.listen(0, '127.0.0.1', resolve))
  const port = probe.address().port
  await new Promise((resolve) => probe.close(resolve))
  return port
}

/**
 * ★ **试连读数的形状：每一档都要带 `models`**（2026-10-14 真机撞上）。
 *
 * 症状：`probeModels` 的**连不上那一档**从前返回 `{ url, ok, status, reason, ...NO_WINDOW }`
 * ——**没有 `models`**。而客户端那份形状守卫按纪律要求它**必须在场**（"缺了它就是'不知道这家
 * 有哪些模型'，而页面会拿它画'照着挑一个'——猜错的方向正好是'这家一个模型都没有'（假的）"）
 * ⇒ 整份读数被判成"认不出的形状" ⇒ 页面把"**连不上**"说成"读不到这份读数"（选型号那一格
 * 点了没反应）。**壳那一侧一直带着它**（`no_window` 里有 `"models": []`）⇒ 两门不同形。
 */
configCase('★ 形状：试连读数**每一档都带 `models`**（连不上那档也一样 —— 两门同形）', async () => {
  const port = await closedPort()
  const reading = await probeModels(`http://127.0.0.1:${port}/v1/chat/completions`, 'sk-x', 'm')
  assert.equal(reading.ok, false, '放掉的端口 ⇒ 连不上')
  assert.equal(reading.reason, 'unreachable')
  assert.equal(reading.status, null)
  assert.ok(
    Array.isArray(reading.models),
    `连不上那一档也必须带 models（缺了整份读数会被判成"认不出"）：${JSON.stringify(reading)}`,
  )
  assert.deepEqual(reading.models, [], '没问到就是空数组（**不是**"缺这一格"）')
  for (const key of ['url', 'ok', 'status', 'reason', 'contextWindow', 'windowField', 'models']) {
    assert.ok(key in reading, `读数缺 ${key}：${JSON.stringify(reading)}`)
  }
})

configCase('探测：复用同一个试连，且**一个字节都不落盘**（清单与绑定前后逐字相同）', async () => {
  const root = scratch('probe-readonly')
  try {
    await withServer(async ({ port, probes }) => {
      // 先正常存一条（让两个文件都在场），再记指纹 —— 否则"没动过"会因为"本来就没有"而恒真。
      await request(port, 'POST', '/home-config', { body: body() })
      const beforeList = fingerprint(listFile(root))
      const beforeBinding = fingerprint(bindingFile(root))
      assert.ok(beforeList !== null && beforeBinding !== null, '前置：两个文件都该在场')

      const res = await request(port, 'POST', PROBE_PATH, { body: probeBody() })
      assert.equal(res.status, 200, res.text)
      assert.equal(res.json.probe.ok, true)
      assert.equal(res.json.probe.url, modelsUrl(probeBody().endpoint), '探的是 <基址>/models（摘掉线协议那段）')
      // **同一处试连**：打了一枪，参数就是请求体里那一份（没有第二套实现）。
      assert.equal(probes.length, 2, '保存一枪 + 探测一枪')
      assert.deepEqual(probes[1], { endpoint: probeBody().endpoint, credential: probeBody().api_key })

      assert.equal(fingerprint(listFile(root)), beforeList, '清单必须一个字节没动')
      assert.equal(fingerprint(bindingFile(root)), beforeBinding, '绑定必须一个字节没动')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测：还没有清单时它**不会顺手创建**文件（探测不产生任何持久事实）', async () => {
  const root = scratch('probe-nofiles')
  try {
    await withServer(async ({ port }) => {
      assert.equal(fingerprint(listFile(root)), null, '前置：清单文件不在')
      const res = await request(port, 'POST', PROBE_PATH, { body: probeBody() })
      assert.equal(res.status, 200, res.text)
      assert.equal(fingerprint(listFile(root)), null, '探测后清单文件仍不该在')
      assert.equal(fingerprint(bindingFile(root)), null, '探测后绑定文件仍不该在')
      assert.deepEqual(listEntries(root), [])
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测：连不上是**读数**不是错误（200 + ok:false + reason），模型目录如实给空', async () => {
  const root = scratch('probe-offline')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', PROBE_PATH, { body: probeBody() })
        assert.equal(res.status, 200, '连不上不改变 HTTP 结论 —— 它是读数，页面据此如实说"这家现在连不上"')
        assert.equal(res.json.probe.ok, false)
        assert.equal(res.json.probe.reason, 'timeout')
        assert.equal(res.json.probe.status, null)
        assert.deepEqual(res.json.probe.models, [], '没连上就没有目录（空数组 ≠ 错误）')
      },
      {
        homeDir: root,
        probe: async (endpoint) => ({
          url: modelsUrl(endpoint),
          ok: false,
          status: null,
          reason: 'timeout',
          ...NO_CATALOG,
        }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测：**形状**不对才 4xx（没给钥匙 / 地址读不懂 / 协议不在闭集），且同样不落盘', async () => {
  const root = scratch('probe-shape')
  try {
    await withServer(async ({ port, probes }) => {
      const noKey = await request(port, 'POST', PROBE_PATH, { body: probeBody({ api_key: '  ' }) })
      assert.equal(noKey.status, 400)
      assert.equal(noKey.json.error, ERRORS.KEY_REQUIRED)

      const badEndpoint = await request(port, 'POST', PROBE_PATH, { body: probeBody({ endpoint: '不是地址' }) })
      assert.equal(badEndpoint.status, 400)
      assert.equal(badEndpoint.json.error, ERRORS.INVALID_ENDPOINT)

      const badProtocol = await request(port, 'POST', PROBE_PATH, { body: probeBody({ protocol: 'grpc' }) })
      assert.equal(badProtocol.status, 400)
      assert.equal(badProtocol.json.error, ERRORS.INVALID_PROTOCOL)

      // 被拒的三次**一次试连都没打**（形状没通过就不该出门），也没写任何东西。
      assert.equal(probes.length, 0)
      assert.equal(fingerprint(listFile(root)), null)
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测：响应里**永远没有 key**（拿一把可辨认的假钥匙逐条扫响应正文）', async () => {
  const root = scratch('probe-canary')
  const canary = 'sk-probe-canary-2f9c'
  try {
    await withServer(async ({ port }) => {
      const probed = await request(port, 'POST', PROBE_PATH, { body: probeBody({ api_key: canary }) })
      const read = await request(port, 'GET', '/home-config')
      for (const [where, res] of [
        ['探测响应', probed],
        ['清单读数', read],
      ]) {
        assert.ok(!res.text.includes(canary), `${where}里不许出现那把钥匙`)
        assert.ok(!res.text.includes('sk-'), `${where}里连一个 sk- 前缀都不该有`)
      }
      // 探测不动清单 ⇒ 这份读数里一条也没有（`keyConfigured` 只能报"配没配"）。
      assert.equal(read.json.keyConfigured, false)
      assert.deepEqual(read.json.entries, [])
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测之后保存走**同一条**路：挑一个模型名 POST /home-config，条目真进清单并可被指定', async () => {
  const root = scratch('probe-then-save')
  try {
    await withServer(async ({ port }) => {
      await request(port, 'POST', PROBE_PATH, { body: probeBody() })
      assert.deepEqual(listEntries(root), [], '前置：探测之后清单仍然是空的')

      const created = await request(port, 'POST', '/home-config', {
        body: body({
          endpoint: probeBody().endpoint,
          apiKey: probeBody().api_key,
          model: 'picked-model',
          contextWindow: 4096,
        }),
      })
      assert.equal(created.status, 200, created.text)
      assert.equal(created.json.bound, created.json.entryId, '存下的第一条默认就指定给 Agent 用（老语义一字未动）')
      assert.equal(listEntries(root)[0].model, 'picked-model', '挑的那个模型名真进了清单')
      assert.equal(listEntries(root)[0].endpoint, probeBody().endpoint, '预设填的地址原样进清单')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 黑盒（**真门进程 ＋ 真 fetch**）＋ 自定义那条路的回归
//
// 与上面那几条的分工：上面注入 `probe` 替身，判的是**接线**（打了几枪、URI 是什么）；
// 这一段**不注入**（用真的 `probeModels`），判的是真 fetch 走完全程之后的**读数形状**
// 与**文件动没动**。两档合起来才等于验收里那句"贴假 key ⇒ 如实失败不落盘"。
// ---------------------------------------------------------------------------

configCase('探测黑盒（真门 + 真 fetch）：对方回 401 ⇒ 如实回报，且一个字节不落盘', async () => {
  const root = scratch('probe-blackbox-401')
  try {
    await withUpstream401(async ({ endpoint }) => {
      await withServer(
        async ({ port }) => {
          const beforeList = fingerprint(listFile(root))
          const beforeBinding = fingerprint(bindingFile(root))
          const res = await request(port, 'POST', PROBE_PATH, { body: probeBody({ endpoint }) })
          assert.equal(res.status, 200, `连不上/被拒都是读数，不是 4xx：${res.text}`)
          assert.equal(res.json.probe.ok, false, '401 不是 2xx ⇒ ok:false')
          assert.equal(res.json.probe.status, 401, '拿到了 HTTP 应答 ⇒ status 就是那个码')
          assert.equal(res.json.probe.reason, null, '拿到了应答 ⇒ reason 是 null（那一格只在"没拿到"时有值）')
          assert.deepEqual(res.json.probe.models, [], '错误页里的字段不是供应商的事实 ⇒ 不读目录')
          assert.ok(res.json.probe.url.endsWith('/v1/models'), `探的是 <基址>/models：${res.json.probe.url}`)
          assert.equal(fingerprint(listFile(root)), beforeList, '清单必须一个字节没动')
          assert.equal(fingerprint(bindingFile(root)), beforeBinding, '绑定必须一个字节没动')
          assert.ok(!res.text.includes(probeBody().api_key), '响应里不许出现那把钥匙')
        },
        { homeDir: root, probe: probeModels },
      )
    })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('探测黑盒：地址指向没人监听的端口 ⇒ `reason=unreachable`（**不是 4xx**），也不落盘', async () => {
  const root = scratch('probe-blackbox-dead')
  try {
    const dead = await closedPort()
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', PROBE_PATH, {
          body: probeBody({ endpoint: `http://127.0.0.1:${dead}/v1/chat/completions` }),
        })
        assert.equal(res.status, 200, '连不上也是读数 ⇒ 200')
        assert.equal(res.json.probe.ok, false)
        assert.equal(res.json.probe.status, null, '没连上 ⇒ 没有 HTTP 码（不拿 0 冒充）')
        assert.equal(res.json.probe.reason, 'unreachable')
        assert.equal(fingerprint(listFile(root)), null, '这一趟没写任何东西')
        assert.equal(fingerprint(bindingFile(root)), null)
      },
      { homeDir: root, probe: probeModels },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('自定义＝降级通道的回归：**手填端点**存的条目与预设那条走同一条路，且真进清单、可被指定', async () => {
  const root = scratch('custom-save')
  try {
    await withServer(async ({ port }) => {
      const custom = 'https://my-own-gateway.example.com/v1/chat/completions'
      const created = await request(port, 'POST', '/home-config', {
        // 手填那条路的形状与预设那条**逐字相同**（页面对两条路调的是同一个 `createHomeSource`），
        // 差别只有端点从哪儿来。`name` 留空 ⇒ 服务用模型名顶（既有语义）。
        body: body({ endpoint: custom, name: '', model: 'my-model', contextWindow: 8192 }),
      })
      assert.equal(created.status, 200, created.text)
      const entries = listEntries(root)
      assert.equal(entries.length, 1)
      assert.equal(entries[0].endpoint, custom, '手填的地址原样进清单（预设层没插手）')
      assert.equal(entries[0].model, 'my-model')
      assert.equal(entries[0].name, 'my-model', '名字留空 ⇒ 用模型名顶（既有语义一字未改）')
      // 清单读数里也要看得见它 —— 这就是"保存后条目真出现在清单"。
      const read = await request(port, 'GET', '/home-config')
      assert.equal(read.json.entries.length, 1)
      assert.equal(read.json.entries[0].endpoint, custom)
      assert.equal(read.json.bound, created.json.entryId, '存下的第一条默认指定给 Agent（绑定流程回归）')
      assert.equal(read.json.state, 'configured')
    }, { homeDir: root })
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 兜底表（2026-09-23 主人裁决）：探测拿不到窗口时，**限家查随包那张表**，而不是直接要人填。
//
// 原理：**窗口是模型的属性，不是用户的配置。** 系统尽力自动拿（探测 → 表兜底），真拿不到才让人填。
// 铁律：查表**必须限家 + 逐字命中** —— 不限定家就等于在猜，而猜的结果不是空，是**一个能骗过人的数字**
// （实测：`mistral-large-latest` 被模糊命中成 32000，真值 256K）。
// ---------------------------------------------------------------------------

/** 最小的兜底表夹具：两条就够验「限家 + 逐字」。 */
const WINDOW_TABLE = createWindowTable({
  deepseek: { 'deepseek-flash': 1000000 },
  mistral: { 'mistral-large-latest': 262144 },
})

/** 探测"答了、但目录里没窗口那两格"——这正是兜底表要补的那一档。 */
const probeNoWindow = () => async (endpoint) => ({
  url: modelsUrl(endpoint),
  ok: true,
  status: 200,
  reason: null,
  ...NO_CATALOG,
})

configCase('★ 兜底表：探测没读到窗口 + 限家命中 ⇒ 自动填上，`windowSource` 如实标 `table`', async () => {
  const root = scratch('table-hit')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'deepseek', model: 'deepseek-flash' }),
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.contextWindow, 1000000, '表里那一条要顶上来')
        // ★ 来源必须如实 —— 帮填 ≠ 权威值（与「预设 ≠ 已配置」同一条纪律）。
        assert.equal(res.json.windowSource, 'table')
        assert.equal(readList(root)[0].context_window, 1000000, '查到的窗口要进清单（Agent 读的就是它）')
      },
      { homeDir: root, probe: probeNoWindow(), windowTable: WINDOW_TABLE },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★★ 兜底表：限家 —— 同一个模型名拿去别的家查必须 miss（窗口是「(家, 模型) 对」的属性）', async () => {
  const root = scratch('table-cross')
  try {
    await withServer(
      async ({ port }) => {
        // 声明是 deepseek 家、却填了 mistral 家的模型名 ⇒ 表里**不许**串过来。
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'deepseek', model: 'mistral-large-latest' }),
        })
        assert.equal(res.status, 400, res.text)
        assert.equal(res.json.error, ERRORS.WINDOW_UNAVAILABLE, '查不到就如实拒绝，不许拿别家的值顶')
        assert.ok(!fs.existsSync(listFile(root)), '没拿到窗口就不落盘')
      },
      { homeDir: root, probe: probeNoWindow(), windowTable: WINDOW_TABLE },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★ 兜底表：没传 provider ⇒ 不查表（向后兼容 —— 老页面不传，行为与从前一字不差）', async () => {
  const root = scratch('table-no-provider')
  try {
    await withServer(
      async ({ port }) => {
        // 没传 provider，但模型名恰好能命中表 ⇒ **仍然不许查**（不知道是哪一家就不查）。
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', model: 'deepseek-flash' }),
        })
        assert.equal(res.status, 400, res.text)
        assert.equal(res.json.error, ERRORS.WINDOW_UNAVAILABLE)
      },
      { homeDir: root, probe: probeNoWindow(), windowTable: WINDOW_TABLE },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★ 兜底表：`provider: custom` ⇒ 不查表（自定义那条路不知道是哪一家，查了会静默给错值）', async () => {
  const root = scratch('table-custom')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'custom', model: 'deepseek-flash' }),
        })
        assert.equal(res.status, 400, res.text)
        assert.equal(res.json.error, ERRORS.WINDOW_UNAVAILABLE)
      },
      { homeDir: root, probe: probeNoWindow(), windowTable: WINDOW_TABLE },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★★ 病因分开：钥匙被对方拒（401）⇒ `credential_rejected`，**不是** `window_unavailable`', async () => {
  const root = scratch('credential-rejected')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'deepseek', model: 'no-such-model' }),
        })
        assert.equal(res.status, 400, res.text)
        // 两者要用户做的事**相反**：这个该去换钥匙（手填窗口照样连不上）。
        assert.equal(res.json.error, ERRORS.CREDENTIAL_REJECTED)
        assert.notEqual(res.json.error, ERRORS.WINDOW_UNAVAILABLE)
        assert.equal(res.json.probe.status, 401, '试连读数照旧交回去（人要看的正是"为什么"）')
      },
      {
        homeDir: root,
        windowTable: WINDOW_TABLE,
        probe: async (endpoint) => ({
          url: modelsUrl(endpoint),
          ok: false,
          status: 401,
          reason: null,
          ...NO_CATALOG,
        }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★ 病因分开的反面：连得上、只是目录里没这一格 ⇒ 仍是 `window_unavailable`（该他手填）', async () => {
  const root = scratch('window-truly-missing')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'deepseek', model: 'no-such-model' }),
        })
        assert.equal(res.status, 400, res.text)
        assert.equal(res.json.error, ERRORS.WINDOW_UNAVAILABLE, '对方答了 200，只是没给窗口 ⇒ 该手填')
        assert.equal(res.json.probe.status, 200)
      },
      { homeDir: root, probe: probeNoWindow(), windowTable: WINDOW_TABLE },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

configCase('★ 表命中时**不看**试连结论：钥匙不对照样能存（「试连失败不拦保存」这条照旧成立）', async () => {
  const root = scratch('table-with-401')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: body({ contextWindow: '', provider: 'deepseek', model: 'deepseek-flash' }),
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.windowSource, 'table')
        assert.equal(res.json.probe.status, 401, '读数照旧如实（页面会说"这一家现在连不上，但配置已经保存了"）')
      },
      {
        homeDir: root,
        windowTable: WINDOW_TABLE,
        probe: async (endpoint) => ({
          url: modelsUrl(endpoint),
          ok: false,
          status: 401,
          reason: null,
          ...NO_CATALOG,
        }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 住户表**每次现问**（2026-10-12）：门起来之后再安的那一支也算数
// ---------------------------------------------------------------------------
//
// 真机撞到的症状（测试区那次 drill）：门在 20:34 起来、第二支 21:2x 才安家 ⇒ **门给的那份逐支
// 读数里永远没有它**（`/home-config` 只报一支），而**新鲜进程**（`node home.js status`）看得到两支
// —— 也就是"门把自己开口那一刻的世界当成了现在"。删掉一支之后反过来：门还报着一行早就没有的住户。
// 权威是磁盘上那份住户表，门只是通道（`agent/src/model_source.rs` 的"不缓存、不派生第三份"同款）。
configCase('★ 住户表**每次现问**：门跑着的时候安第二支 ⇒ 同一台门的读数里就有它（读 ＋ 写都算）', async () => {
  const root = scratch('resident-live')
  const second = path.join(root, 'agents', 'a-2')
  try {
    fs.mkdirSync(path.join(root, 'env'), { recursive: true })
    modelSources.writeBinding(path.join(root, 'env'), 'ms-1')
    modelSources.writeList(root, [
      {
        id: 'ms-1',
        name: '主钥匙',
        endpoint: 'https://a.invalid/v1/chat/completions',
        api_key: 'k1',
        model: 'm1',
        context_window: 8192,
      },
      {
        id: 'ms-2',
        name: '备用',
        endpoint: 'https://b.invalid/v1/chat/completions',
        api_key: 'k2',
        model: 'm2',
        context_window: 4096,
      },
    ])
    // **会变的**住户表：`live` 就是"磁盘上此刻住着谁"的替身（生产里由 `runtimeResidents` 现读）。
    let live = [{ agentId: 'a-1', envDir: path.join(root, 'env') }]
    await withServer(
      async ({ port }) => {
        const first = await request(port, 'GET', '/home-config')
        assert.equal(first.json.boundPerResident.length, 1, '前提：此刻只有一支')
        assert.equal(first.json.bindingDivergent, false)

        // **门跑着的时候**安第二支：建环境根 ＋ 它自己那份绑定（这一支用 ms-2 ⇒ 两支不一样）
        fs.mkdirSync(second, { recursive: true })
        modelSources.writeBinding(second, 'ms-2')
        live = [...live, { agentId: 'a-2', envDir: second }]

        const after = await request(port, 'GET', '/home-config')
        assert.equal(after.json.boundPerResident.length, 2, '第二支必须在读数里（从前它到门退出都不出现）')
        assert.deepEqual(
          after.json.boundPerResident.map((row) => row.bound),
          ['ms-1', 'ms-2'],
        )
        assert.equal(after.json.bindingDivergent, true, '两支各绑不同 ⇒ 如实标出来')
        assert.equal(after.json.bound, 'ms-1', '整机那一格仍是第一支那条（口径未变）')

        // **写**也落在现问出来的每一支上：换绑 ⇒ 第二支那份也要跟着换。
        await request(port, 'PUT', '/home-binding', { body: { source: 'ms-1' } })
        assert.equal(modelSources.readBinding(second).source, 'ms-1', '换绑要落在第二支上')
        assert.equal(modelSources.readBinding(path.join(root, 'env')).source, 'ms-1')
      },
      { homeDir: root, residents: () => live },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **住户表为空是一个事实，不是"没给"**（2026-10-12）：`() => []` 与"没有住户表"必须分开 ——
//
// 症状（主人截图那台机器）：真家**一支住户都没有**，壳那侧的门给 **0 行**逐支读数，而 JS 门（web 形态）
// 给 **1 行 `agentId=null`** 的**合成住户**（那条 `length > 0 ? … : [老家那一支]` 的兜底）。
// 危害不只是"两个门不同形"：`PUT /home-binding` 于是**有目标**了 —— 它会去写
// `<家>/env/model_binding.json`（`writeBinding` 第一句就 `mkdirSync`）**一份不属于任何人的绑定**，
// 而快照还报"已指定"。⇒ 读：0 行；写：**明确拒绝**（`no_resident`），一个字节都不落。
configCase('★ 0 住户：逐支读数 **0 行**（不合成老家那一支），换绑/解绑**明确拒绝**且不落一个字节', async () => {
  const root = scratch('no-resident')
  try {
    await withServer(
      async ({ port }) => {
        const res = await request(port, 'GET', '/home-config')
        assert.equal(res.json.boundPerResident.length, 0, '0 住户就是 0 行（壳那侧一直是 0 行，两门要同形）')
        assert.equal(res.json.bindingDivergent, false)
        assert.equal(res.json.bound, null)

        const put = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-1' } })
        assert.equal(put.status, 409, JSON.stringify(put.json))
        assert.equal(put.json.error, 'no_resident')
        // 解绑那一半同样没有目标（"解除"也是住户级动作）。
        const clear = await request(port, 'PUT', '/home-binding', { body: { source: '' } })
        assert.equal(clear.status, 409, JSON.stringify(clear.json))
        assert.equal(clear.json.error, 'no_resident')

        assert.ok(
          !fs.existsSync(path.join(root, 'env')),
          '不许造出 <家>/env —— 那正是"合成一支住户"留下的孤儿目录',
        )
      },
      { homeDir: root, residents: () => [] },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **一支一支地换**（2026-10-12 住户级覆盖）：`agentId` 给了就**只写那一支** ——
//
// 症状：一个家住两支，主人在机器页上只能"每一支都换成这条"（家级默认），**没法**让 A 用 1 号、
// B 用 2 号。而绑定本来就是住户级事实（一支一份 `model_binding.json`）—— 缺的只是这条路。
// 判据（四条，一个不压）：① 只动点名的那一支（另一支一个字节不变）；
// ② **点名一支还没住户化的** ⇒ 落在**她将来会住的那个环境根**上（2026-10-14"选择先于接上"；
//    ⚠ 这一格**从前是拒绝**，翻转的记账写在用例里那一段注释上）；
// ③ 点名一个**形状不合法**的 ⇒ `400 bad_agent_id`，一个字节都不写（那一格会变成目录名）；
// ④ 不给 `agentId` ⇒ 与从前一字不差（每一支都换）。
configCase('★ 住户级换绑：只动点名的那一支；还没住户化的落在她的环境根上；形状不合法就拒', async () => {
  const root = scratch('per-resident')
  const first = path.join(root, 'agents', 'a-1')
  const second = path.join(root, 'agents', 'a-2')
  try {
    fs.mkdirSync(first, { recursive: true })
    fs.mkdirSync(second, { recursive: true })
    modelSources.writeBinding(first, 'ms-1')
    modelSources.writeBinding(second, 'ms-1')
    modelSources.writeList(root, [
      {
        id: 'ms-1',
        name: '主钥匙',
        endpoint: 'https://a.invalid/v1/chat/completions',
        api_key: 'k1',
        model: 'm1',
        context_window: 8192,
      },
      {
        id: 'ms-2',
        name: '备用',
        endpoint: 'https://b.invalid/v1/chat/completions',
        api_key: 'k2',
        model: 'm2',
        context_window: 4096,
      },
    ])
    const live = [
      { agentId: 'a-1', envDir: first },
      { agentId: 'a-2', envDir: second },
    ]
    await withServer(
      async ({ port }) => {
        // ① 只给 a-2 换 ⇒ 只有它那份文件变了。
        const one = await request(port, 'PUT', '/home-binding', {
          body: { source: 'ms-2', agentId: 'a-2' },
        })
        assert.equal(one.status, 200, JSON.stringify(one.json))
        assert.equal(modelSources.readBinding(second).source, 'ms-2', '点名的那一支要换过去')
        assert.equal(modelSources.readBinding(first).source, 'ms-1', '没点名的那一支一个字节都不许动')
        assert.deepEqual(
          one.json.boundPerResident.map((row) => row.bound),
          ['ms-1', 'ms-2'],
        )
        assert.equal(one.json.bindingDivergent, true, '各支不一致 ⇒ 如实标出来')

        // ② **点名一支还没住户化的**（2026-10-14「选择先于接上」）⇒ 落在**她将来会住的那个
        //    环境根**上（`<家>/agents/<id>/`），别人一个字节不动。
        //
        //    ⚠ **判据翻转的记账**：这一格从前是 `409 no_such_resident`（"这台机器上没有这一支
        //    ⇒ 拒"）。翻转的理由不是放宽，是**那条判据缺了一个真处境**：绑定本来就住在"她的
        //    环境根"里，而环境根要等接她第 2 步才建 ⇒ "先给她选好、再接上"这条路在页面上
        //    走不通（2026-10-14 主人撞上：机器上有两条来源，接她时按"选哪条 = 花谁的钱"
        //    停在原地，而选的那一页要求她已经在机器上 —— 两边互等）。
        //    新边界：**点名 + 形状合法 ⇒ 写她那个环境根**；形状不合法 ⇒ `400 bad_agent_id`
        //    （见 ②b，它守着这一格会变成目录名那件事）。老消费者不少什么：`no_such_resident`
        //    照旧留在封闭表里（**旧版的门**在远程那台还这么答）。
        const fresh = path.join(root, 'agents', 'a-9')
        const seeded = await request(port, 'PUT', '/home-binding', {
          body: { source: 'ms-2', agentId: 'a-9' },
        })
        assert.equal(seeded.status, 200, JSON.stringify(seeded.json))
        assert.equal(
          modelSources.readBinding(fresh).source,
          'ms-2',
          '先选好的那一笔要落在她将来会住的环境根上',
        )
        assert.equal(fs.existsSync(path.join(fresh, 'agent.conf')), false, '空目录不许变成住户')
        assert.equal(modelSources.readBinding(first).source, 'ms-1', '别人一个字节都不许动')
        assert.equal(modelSources.readBinding(second).source, 'ms-2')
        // 读面：她进 `boundUnhomed`（"先选好了"），**不进** `boundPerResident`（"住户"）。
        assert.deepEqual(
          seeded.json.boundUnhomed.map((row) => [row.agentId, row.bound]),
          [['a-9', 'ms-2']],
          '先选好的那一支要如实读回来（否则页面会把"选过了"说成"没指定"）',
        )
        assert.deepEqual(
          seeded.json.boundPerResident.map((row) => row.agentId),
          ['a-1', 'a-2'],
          '她不是住户 ⇒ 不许混进住户那一格（那是假话）',
        )

        // ②b **形状不合法** ⇒ 400，且**连目录都不建**（这一格会变成目录名 ⇒ 不预检就是穿越）。
        //     ⚠ 空串**不在**这张表里：它是"没点名"（家级默认每一支都换，见 ④），不是形状错。
        for (const bad of ['../evil', 'a/b', 'a b', '.', '..', 'a:b']) {
          const refused = await request(port, 'PUT', '/home-binding', {
            body: { source: 'ms-2', agentId: bad },
          })
          assert.equal(refused.status, 400, `形状不合法的 ${JSON.stringify(bad)} 必须拒`)
          assert.equal(refused.json.error, ERRORS.BAD_AGENT_ID)
        }
        assert.equal(fs.existsSync(path.join(root, 'evil')), false, '不许拼出 root 之外的路径')
        assert.equal(
          fs.existsSync(path.join(root, 'agents', '..', 'evil')),
          false,
          '不许拼出 root 之外的路径（经 agents/ 那条）',
        )
        assert.equal(modelSources.readBinding(first).source, 'ms-1', '被拒的那几笔一个字节都没写')
        assert.equal(modelSources.readBinding(second).source, 'ms-2')

        // ③ 只给 a-2 解除 ⇒ 只有它回到"没指定"。
        const cleared = await request(port, 'PUT', '/home-binding', {
          body: { source: '', agentId: 'a-2' },
        })
        assert.equal(cleared.status, 200, JSON.stringify(cleared.json))
        assert.equal(modelSources.readBinding(second).state, 'absent')
        assert.equal(modelSources.readBinding(first).source, 'ms-1')

        // ④ 不给 `agentId` ⇒ 家级默认（每一支都换），与从前一字不差。
        const all = await request(port, 'PUT', '/home-binding', { body: { source: 'ms-2' } })
        assert.equal(all.status, 200, JSON.stringify(all.json))
        assert.equal(modelSources.readBinding(first).source, 'ms-2')
        assert.equal(modelSources.readBinding(second).source, 'ms-2')
        assert.equal(all.json.bindingDivergent, false)
      },
      { homeDir: root, residents: () => live },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **支级覆盖那两格**（2026-10-14）："这一支具体用哪个型号 / 什么思考档"。
//
// 症状：运行时**一直**认它俩（`agent/src/model_source.rs` 的 `apply_binding_overrides`：绑定的
// 型号/档位赢过条目里的），而**没有任何写者** —— 页面上只能靠"再建一条只差型号的条目"来间接
// 表达"换个型号"（主人真机上就是这么干的：`ms-1` / `ms-2` 同端点同钥匙、只差型号）。
//
// 判据（四条，一个不压）：① 点名那一支 ⇒ 写进去、**读面如实读回来**（两个行集都带那两格）；
// ② `null` = 清掉那一格（回到"用条目里的"），**另一格不动**；③ 形状不合法 ⇒ 400 且**一个字节
// 都不写**（写进去的值会让宿主拒绝启动）；④ 没点名（家级默认）或"解除绑定"带它 ⇒ 400
// ——那两格是**支级事实**，"每一支都用同一个型号"不是任何人要的。
configCase('★ 支级覆盖：写型号/思考档 ⇒ 读数如实带回来；`null` 清掉；坏形状 400 且不写', async () => {
  const root = scratch('overrides')
  const envDir = path.join(root, 'agents', 'a-1')
  try {
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(path.join(envDir, 'identity.json'), '{"agent":"a-1"}')
    modelSources.writeBinding(envDir, 'ms-1')
    modelSources.writeList(root, [
      { id: 'ms-1', name: '主钥匙', endpoint: 'https://a.invalid/v1/chat/completions', api_key: 'k1', model: 'm1', context_window: 8192 },
      { id: 'ms-2', name: '备用', endpoint: 'https://b.invalid/v1/chat/completions', api_key: 'k2', model: 'm2', context_window: 4096 },
    ])
    await withServer(
      async ({ port }) => {
        const row = async () => {
          const res = await request(port, 'GET', '/home-config')
          return res.json.boundPerResident.find((each) => each.agentId === 'a-1')
        }
        /** 逐支那一行的**读数**（把见证那一格摘出来单看 —— 它带一个真实的时刻）。 */
        const readout = async () => {
          const { witness, ...rest } = await row()
          return { rest, witness }
        }
        // ① 两格都写 ⇒ 文件里是它们，读面也带回来（页面才敢说"具体用哪个型号"）。
        const saved = await request(port, 'PUT', '/home-binding', {
          body: { source: 'ms-1', agentId: 'a-1', model: 'deepseek-v4.1-flash', reasoningEffort: 'max' },
        })
        assert.equal(saved.status, 200, saved.text)
        assert.deepEqual(JSON.parse(fs.readFileSync(modelSources.bindingPath(envDir), 'utf8')), {
          source: 'ms-1',
          model: 'deepseek-v4.1-flash',
          reasoning_effort: 'max',
        })
        const first = await readout()
        assert.deepEqual(first.rest, {
          agentId: 'a-1',
          state: 'bound',
          bound: 'ms-1',
          model: 'deepseek-v4.1-flash',
          reasoningEffort: 'max',
        })
        // ★ 见证（2026-10-18）：这一笔是**从这道门**出去的 ⇒ 读数说"门写过，有时刻"。
        assert.equal(first.witness.state, 'same')
        assert.ok(Number.isInteger(first.witness.atMs) && first.witness.atMs > 0, '有时刻')
        assert.equal(first.witness.detail, null)

        // ② `null` 只清那一格（另一格原样）——"回到用条目里的型号"。
        const cleared = await request(port, 'PUT', '/home-binding', {
          body: { source: 'ms-1', agentId: 'a-1', model: null },
        })
        assert.equal(cleared.status, 200, cleared.text)
        assert.deepEqual(JSON.parse(fs.readFileSync(modelSources.bindingPath(envDir), 'utf8')), {
          source: 'ms-1',
          reasoning_effort: 'max',
        })
        assert.equal((await row()).model, null, '清掉之后读面说得出"没覆盖"')

        // ③ 坏形状 ⇒ 400，且**一个字节都不写**（每一格各自一条）。
        const before = fingerprint(modelSources.bindingPath(envDir))
        for (const [key, value, code] of [
          ['model', '', ERRORS.INVALID_MODEL],
          ['model', '   ', ERRORS.INVALID_MODEL],
          ['model', 'x'.repeat(201), ERRORS.INVALID_MODEL],
          ['model', 42, ERRORS.INVALID_MODEL],
          ['reasoningEffort', 'turbo', ERRORS.INVALID_EFFORT],
          ['reasoningEffort', 'MAX', ERRORS.INVALID_EFFORT],
          ['reasoningEffort', 3, ERRORS.INVALID_EFFORT],
        ]) {
          const bad = await request(port, 'PUT', '/home-binding', {
            body: { source: 'ms-1', agentId: 'a-1', [key]: value },
          })
          assert.equal(bad.status, 400, `${key}=${JSON.stringify(value)} 该被拒：${bad.text}`)
          assert.equal(bad.json.error, code)
          assert.equal(fingerprint(modelSources.bindingPath(envDir)), before, '被拒的那几笔一个字节都没写')
        }

        // ④ 没点名（家级默认）⇒ 400；"解除绑定"带它 ⇒ 400（自相矛盾：文件要删了，覆盖没有落点）。
        for (const payload of [
          { source: 'ms-1', model: 'x' },
          { source: '', agentId: 'a-1', model: 'x' },
        ]) {
          const refused = await request(port, 'PUT', '/home-binding', { body: payload })
          assert.equal(refused.status, 400, JSON.stringify(payload))
          assert.equal(refused.json.error, ERRORS.BAD_REQUEST)
        }
        assert.equal(fingerprint(modelSources.bindingPath(envDir)), before, '那两笔也没写')
      },
      { homeDir: root, residents: () => [{ agentId: 'a-1', envDir }] },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ★ **只读目录**（2026-10-14）："**这条钥匙**能调哪些型号"——请求体只有条目 `id`，
// 钥匙用清单里**存着的那把**（页面手里**永远没有**它：key 只到那台机器为止）。
//
// 判据与探测那一格**逐条相同**：一个字节都不落盘、响应里没有 key、连不上是读数不是错误；
// 外加两条这一格特有的：`id` 认不出 ⇒ 404、没给 `id` ⇒ 400（都不猜）。
configCase('★ 只读目录：用存着的钥匙问型号 ⇒ 200 且不落盘、不吐 key；id 认不出 404', async () => {
  const root = scratch('catalog')
  const envDir = path.join(root, 'agents', 'a-1')
  try {
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(path.join(envDir, 'identity.json'), '{"agent":"a-1"}')
    modelSources.writeBinding(envDir, 'ms-1')
    modelSources.writeList(root, [
      {
        id: 'ms-1',
        name: '主钥匙',
        endpoint: 'https://a.invalid/v1/chat/completions',
        api_key: 'sk-catalog-canary-7d31',
        model: 'm1',
        context_window: 8192,
      },
    ])
    await withServer(
      async ({ port, probes }) => {
        const beforeList = fingerprint(modelSources.listPath(root))
        const beforeBinding = fingerprint(modelSources.bindingPath(envDir))
        const res = await request(port, 'POST', CATALOG_PATH, { body: { id: 'ms-1' } })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.probe.ok, true)
        assert.equal(res.json.probe.url, modelsUrl('https://a.invalid/v1/chat/completions'))
        // **钥匙是清单里那把**（不是请求体里给的任何东西；请求体里根本没有 key 这一格）。
        assert.deepEqual(probes.at(-1), {
          endpoint: 'https://a.invalid/v1/chat/completions',
          credential: 'sk-catalog-canary-7d31',
        })
        // **响应里没有 key**，也**没落一个字节**。
        assert.ok(!res.text.includes('sk-catalog-canary-7d31'), `key 不许进响应：${res.text}`)
        assert.equal(fingerprint(modelSources.listPath(root)), beforeList, '清单必须一个字节没动')
        assert.equal(fingerprint(modelSources.bindingPath(envDir)), beforeBinding, '绑定必须一个字节没动')

        // `id` 认不出 ⇒ 404（与"改一条点名了不存在的 id"同一个码）；没给 id ⇒ 400。
        const missing = await request(port, 'POST', CATALOG_PATH, { body: { id: 'ms-9' } })
        assert.equal(missing.status, 404)
        assert.equal(missing.json.error, ERRORS.ENTRY_NOT_FOUND)
        const empty = await request(port, 'POST', CATALOG_PATH, { body: {} })
        assert.equal(empty.status, 400)
        assert.equal(empty.json.error, ERRORS.BAD_REQUEST)
      },
      { homeDir: root, residents: () => [{ agentId: 'a-1', envDir }] },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ── 2026-10-17：**页面不认识的那一格，不许在保存里被删掉** ─────────────────────────────
//
// 病的形状：门是**整条替换**，而来件是页面按它认识的几格拼的 ⇒ 页面还不认识的一格会在每一次
// 保存里**静默消失**。同一个病在本仓已经发生过两次（`reasoning_effort` 那次"页面保存抹掉清单
// 那一格"、2026-10-09 那次"重新绑定抹掉那两格"）——而它的后果**没有任何报错**：
// 用户以为配好了，而她静默地重新读不动图片。
//
// 判据：**来件没声明 ⇒ 从旧条目原样带过来；来件显式声明了（哪怕 `false`）⇒ 以它为准。**
// 这两半必须一起守——只守前一半，会把"明确关掉视觉"变成一个关不掉的动作。
test('页面不认识的那一格，保存时不许被删掉', () => {
  const modelSources = require('../model-sources')
  const root = scratch('keep-vision')
  try {
    const entry = {
      id: 'ms-1',
      name: '会看图的那条',
      endpoint: 'https://api.example.invalid/chat/completions',
      api_key: 'sk-x',
      model: 'm',
      context_window: 1000,
    }
    // 盘上先有一条**声明了视觉**的条目（手写的，或将来页面写的）。
    modelSources.writeList(root, [{ ...entry, vision: true }])
    assert.equal(modelSources.readList(root).entries[0].vision, true, '前提：盘上那一格在')

    // 页面保存这一条——**它还不认识 `vision`，所以来件里没有这一格**。
    assert.equal(modelSources.upsertEntry(root, { ...entry }).ok, true)
    assert.equal(
      modelSources.readList(root).entries[0].vision,
      true,
      '来件没说的那一格必须从旧条目带过来（否则她静默地重新读不动图片）',
    )

    // 来件**显式**说了不支持 ⇒ 以它为准（"没说"与"说了 false"是两件事）。
    assert.equal(modelSources.upsertEntry(root, { ...entry, vision: false }).ok, true)
    assert.equal(
      modelSources.readList(root).entries[0].vision,
      false,
      '显式声明以它为准（否则"关掉视觉"会变成一个关不掉的动作）',
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

// ── 2026-10-17：**门的白名单也要认那一格，而且必须是三态** ────────────────────────────────
//
// 为什么这条必须**走门**（而不是直接测 `upsertEntry`）：那是**两层**。模块层的保险已经有用例
// 钉着；而门在它之前还有**一份按名重建的白名单**——漏了那一层，页面上的勾会**静默不生效**
// （存下去的还是旧值，没有任何报错）。
//
// 三态的判据：**没说 ⇒ 带过来；说了 ⇒ 以它为准；说错了 ⇒ 明确拒绝。**
//
// ⚠ 这条用例**自带服务器**（不借 `withServer`/`request` 那套夹具）：它要盯的是门**自己**的
// 白名单与三态，夹具那一层不是被测对象——第一版借夹具写的时候红在一个夹具细节上，
// 而同一份来件直接打门是 200/200/400（判据全对）。
test('视觉那一格：门认它，而且是三态（没说带过来 / 说了以它为准 / 说错了拒绝）', async () => {
  const root = scratch('vision-tri-state')
  const server = createConfigServer(
    {
      homeDir: root,
      probe: async (endpoint) => ({
        url: modelsUrl(endpoint),
        ok: true,
        status: 200,
        reason: null,
        contextWindow: 4096,
        windowField: 'context_length',
      }),
    },
    {},
  )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  try {
    // ⚠ **新建用 `POST`、改一条用 `PUT`**（`pickEntryId` 的文档：`POST` 是新建，调用方指定 id
    // **一律拒绝**——"那是新建，不是覆盖"）。第一版用 `POST` 带 id ⇒ 400，那是**门对、用例错**。
    const send = async (method, extra) => {
      const res = await fetch(`http://127.0.0.1:${port}/home-config`, {
        method,
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ ...body({ contextWindow: 4096 }), ...extra }),
      })
      return { status: res.status, json: await res.json() }
    }

    // ① 说了支持 ⇒ 落盘 true（新建）。
    // ⚠ 之后每次都点名同一个 `id`：不点名就是"改当前绑定的那一条"或新建，
    // 而 `readList(root)[0]` 读的是**最旧**那条，测的就不是"同一格被改"了（三次都栽在这一类上）。
    const created = await send('POST', { vision: true })
    assert.equal(created.status, 200, JSON.stringify(created.json))
    const id = created.json.entryId
    assert.ok(id, '门要回它发的那条 id')
    const save = (extra) => send('PUT', { id, ...extra })

    assert.equal(readList(root)[0].vision, true, '门要把它透到清单里')

    // ② **没说**（老页面）⇒ 从旧条目带过来，不许删。
    assert.equal((await save({})).status, 200)
    assert.equal(
      readList(root)[0].vision,
      true,
      '来件没说那一格 ⇒ 门不许把它压成 false（那会让页面上的勾静默失效）',
    )

    // ③ 说了不支持 ⇒ 以它为准。
    assert.equal((await save({ vision: false })).status, 200)
    assert.equal(readList(root)[0].vision, false, '显式声明以它为准')

    // ④ 说错了 ⇒ **明确拒绝**，不静默当成"没说"（那会把一次笔误读成"用户没打算说"）。
    const bad = await save({ vision: 'yes' })
    assert.equal(bad.status, 400)
    assert.equal(bad.json.error, ERRORS.INVALID_VISION)
    assert.equal(readList(root)[0].vision, false, '被拒的那一次一个字节都不动')
  } finally {
    await new Promise((resolve) => server.close(resolve))
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('（用例计数）', () => {
  assert.ok(cases > 0, '模型门用例数必须大于零')
  console.log(`      config-server 用例数：${cases}`)
})