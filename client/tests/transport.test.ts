// **出网窄口**（`api/transport.ts`）的机械判据（2026-09-29 立）。
//
// 这一刀把"请求由谁发出"收成一个可替换的东西 —— 而它在 Celira 里**是有语义的**：
// 浏览器的同源策略只加在*网页*这个身份上，请求改由壳的 Rust 侧发出时那条规矩不参与
// ⇒「app 连任意地址的世界」不需要 im 服务端新增任何东西。
//
// 判据要钉住四件事：
//   1. **换实现真的生效**（`setTransport` 之后 `sendRequest` 走新的那一份）；
//   2. **不缓存全局 `fetch`** —— 加载期绑定会让"测试打桩 globalThis.fetch"静默失效
//      （本仓实测踩过：一次收口让 17 条用例表面通过、实际什么都没验）；
//   3. **没有内置 fetch 时明确失败**，不是 `undefined is not a function`；
//   4. **业务代码没有绕过这个口的 `fetch`**（源码守卫，正反例都认）。
//
// 运行：cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import {
  activeTransport,
  browserTransport,
  sendRequest,
  setTransport,
  TransportFailure,
  type Transport,
} from '../src/api/transport.ts'

/** 门禁不许空跑（与仓库其余门禁同一条纪律）。 */
let completed = 0
const DECLARED_CASES = 8

function transportCase(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    await fn()
    completed += 1
  })
}

/** 一份记录调用参数的假实现（不动全局）。 */
function recordingTransport(): { transport: Transport; calls: Array<Record<string, unknown>> } {
  const calls: Array<Record<string, unknown>> = []
  const transport: Transport = {
    async send(request) {
      calls.push(request as unknown as Record<string, unknown>)
      return {
        status: 200,
        ok: true,
        header: () => null,
        json: async () => ({ ok: true }),
        blob: async () => new Blob([]),
      }
    },
  }
  return { transport, calls }
}

transportCase('换实现真的生效：setTransport 之后 sendRequest 走新的那一份', async () => {
  const original = activeTransport()
  const { transport, calls } = recordingTransport()
  try {
    setTransport(transport)
    const res = await sendRequest({ url: '/v1/home', method: 'GET', headers: {} })
    assert.equal(res.status, 200)
    assert.equal(calls.length, 1, '新实现必须被调用一次')
    assert.equal(calls[0]!['url'], '/v1/home')
  } finally {
    setTransport(original)
  }
})

transportCase('请求形状原样交到实现手上（方法 / 头 / 体 / 取消信号都不吞）', async () => {
  const original = activeTransport()
  const { transport, calls } = recordingTransport()
  const controller = new AbortController()
  try {
    setTransport(transport)
    await sendRequest({
      url: 'https://example.test/v1/x',
      method: 'PUT',
      headers: { Authorization: 'Bearer t' },
      body: '{"a":1}',
      signal: controller.signal,
    })
    const got = calls[0]!
    assert.equal(got['method'], 'PUT')
    assert.deepEqual(got['headers'], { Authorization: 'Bearer t' })
    assert.equal(got['body'], '{"a":1}')
    assert.equal(got['signal'], controller.signal, '取消信号必须原样传下去（家通道靠它超时）')
  } finally {
    setTransport(original)
  }
})

transportCase('**不缓存全局 fetch**：换掉 globalThis.fetch 之后，同一个实现走的是新的那一份', async () => {
  const originalFetch = globalThis.fetch
  const seen: string[] = []
  try {
    const impl = browserTransport()
    globalThis.fetch = (async (url: string | URL) => {
      seen.push(String(url))
      return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } })
    }) as typeof fetch
    await impl.send({ url: '/a', method: 'GET', headers: {} })

    // 再换一次：如果实现在构造时把 fetch 绑死了，这一次仍会打到上面那个桩上。
    globalThis.fetch = (async (url: string | URL) => {
      seen.push(`second:${String(url)}`)
      return new Response('{}', { status: 200 })
    }) as typeof fetch
    await impl.send({ url: '/b', method: 'GET', headers: {} })

    assert.deepEqual(seen, ['/a', 'second:/b'], '第二次必须打到新的那一份上（证明没有加载期绑定）')
  } finally {
    globalThis.fetch = originalFetch
  }
})

transportCase('没有内置 fetch ⇒ **明确失败**（一句能读懂的话，不是 undefined is not a function）', async () => {
  const originalFetch = globalThis.fetch
  try {
    // @ts-expect-error 故意制造"这个平台没有 fetch"
    delete globalThis.fetch
    await assert.rejects(
      () => browserTransport().send({ url: '/a', method: 'GET', headers: {} }),
      /没有内置 fetch/,
    )
  } finally {
    globalThis.fetch = originalFetch
  }
})

transportCase('响应只暴露那四个：status / ok / header / json / blob（多一个就多一份实现补不齐的风险）', async () => {
  const originalFetch = globalThis.fetch
  try {
    globalThis.fetch = (async () =>
      new Response('{"n":1}', {
        status: 201,
        headers: { 'x-request-id': 'rid-1' },
      })) as typeof fetch
    const res = await browserTransport().send({ url: '/a', method: 'GET', headers: {} })
    assert.equal(res.status, 201)
    assert.equal(res.ok, true)
    assert.equal(res.header('x-request-id'), 'rid-1')
    assert.deepEqual(await res.json(), { n: 1 })

    // 体只能消费一次（`Response` 的规矩，不是本口的规矩）——所以第二条走一次新请求。
    const res2 = await browserTransport().send({ url: '/b', method: 'GET', headers: {} })
    assert.ok((await res2.blob()) instanceof Blob)
  } finally {
    globalThis.fetch = originalFetch
  }
})

// ★ 2026-10-08 加两条：**"发不出去"这件事的分类**（那一刀把库的原文挡在界面之外）。
// 判据来自 `app/README.md` §5 第 15 行那条缺陷的现场：壳的 Rust 侧连不上时抛的是
// `error sending request for url (…)`，而它一路原样渲染到了登录页上。

transportCase('实现抛错 ⇒ 折成 `TransportFailure`（带稳定分类；原文进 `reason`，不上屏）', async () => {
  const original = activeTransport()
  const raw = 'error sending request for url (http://127.0.0.1:8180/v1/me)'
  try {
    setTransport({
      async send() {
        throw new Error(raw)
      },
    })
    await assert.rejects(
      () => sendRequest({ url: 'http://127.0.0.1:8180/v1/me', method: 'GET', headers: {} }),
      (error: unknown) => {
        assert.ok(error instanceof TransportFailure, '必须是这一档，不是原样穿过去')
        assert.equal(error.code, 'transport_unreachable', '稳定分类是它与文案表之间的唯一接口')
        assert.equal(String(error.reason), `Error: ${raw}`, '原文要留得住（诊断用）')
        // ★ 界面读的是分类与人话，**不是它自己的 message**
        assert.ok(
          !error.message.includes('error sending request'),
          `库的原文不许成为它自己的话：${error.message}`,
        )
        return true
      },
    )
  } finally {
    setTransport(original)
  }
})

transportCase('取消 / 超时**原样放行**（折了它，调用方就分不出"等太久"与"够不着"）', async () => {
  const original = activeTransport()
  try {
    for (const name of ['AbortError', 'TimeoutError']) {
      const cancellation = Object.assign(new Error('x'), { name })
      setTransport({
        async send() {
          throw cancellation
        },
      })
      await assert.rejects(
        () => sendRequest({ url: '/a', method: 'GET', headers: {} }),
        (error: unknown) => {
          assert.equal(error, cancellation, `${name} 必须原样穿过去（一个字节都不许改）`)
          return true
        },
      )
    }
  } finally {
    setTransport(original)
  }
})

transportCase('源码守卫：业务代码里没有绕过这个口的裸 fetch（这个文件与 transport.ts 除外）', () => {
  const srcRoot = fileURLToPath(new URL('../src', import.meta.url))
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const full = join(dir, name)
      if (statSync(full).isDirectory()) return walk(full)
      return /\.tsx?$/.test(name) ? [full] : []
    })

  const offenders: string[] = []
  for (const file of walk(srcRoot)) {
    if (file.endsWith('transport.ts')) continue // 这一份就是实现本身
    const source = readFileSync(file, 'utf8')
    const lines = source.split('\n')
    lines.forEach((line, index) => {
      if (/^\s*[*/]/.test(line)) return // 注释里提到 fetch 不算
      // 裸 fetch(...) 调用：`sendRequest(` / `doFetch(` / `globalThis.fetch(` 都不算
      if (/(?<![\w.])fetch\s*\(/.test(line) && !/doFetch|globalThis\.fetch|typeof fetch/.test(line)) {
        offenders.push(`${file.replace(srcRoot, 'src')}:${index + 1}`)
      }
    })
  }
  assert.deepEqual(
    offenders,
    [],
    `出网必须唯一经由 api/transport.ts；这些地方绕过了它：${offenders.join(', ')}`,
  )
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.equal(completed, DECLARED_CASES, `用例数 ${completed} 与声明的 ${DECLARED_CASES} 不符`)
  assert.ok(completed > 0, '本文件一条用例都没跑（空跑不算过）')
})
