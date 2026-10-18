// **壳的出网接线**（`api/shell.ts`）的机械判据（2026-09-29 立）。
//
// 这一刀要钉住的是**一条安全与连通性的边界**，不是"跨平台适配"：
// web 版的请求由 webview 发出 ⇒ 受同源策略管；壳里换成 Rust 侧 ⇒ 那条规矩不参与
// ⇒「连任意地址的世界」不需要 im 服务端新增任何东西。
//
// 判据四件事：
//   1. **浏览器里必须什么也不做** —— 判据是"有没有壳的内部注入"，不是平台名；
//   2. **相对路径在壳里必须响亮地失败**（壳里没有"页面来源"可解析）；
//      静默打到一个与用户意图无关的地址，比报错糟得多；
//   3. **请求形状原样交到 Rust 侧**（方法 / 头 / 体 / **取消信号**都不吞 —— 家通道靠它超时）；
//   4. **响应只暴露那四个**（status / ok / header / json / blob）。
//
// 运行：cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { isTauriRuntime, resolveShellUrl, shellTransport } from '../src/api/shell.ts'

/** 门禁不许空跑（与仓库其余门禁同一条纪律）。 */
let completed = 0
const DECLARED_CASES = 6

function shellCase(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    await fn()
    completed += 1
  })
}

shellCase('判据是"有没有壳的内部注入"（不是平台名、不是构建变量）', () => {
  assert.equal(isTauriRuntime({}), false, '空环境 = 浏览器')
  assert.equal(isTauriRuntime({ __TAURI_INTERNALS__: undefined }), false)
  assert.equal(isTauriRuntime({ __TAURI_INTERNALS__: 'nope' }), false, '形状不对也不算')
  assert.equal(isTauriRuntime({ __TAURI_INTERNALS__: {} }), true)
})

shellCase('绝对地址原样用（世界与家都是"绝对地址 + 凭据"）', () => {
  assert.equal(resolveShellUrl('http://127.0.0.1:8080/v1/home'), 'http://127.0.0.1:8080/v1/home')
  assert.equal(resolveShellUrl('https://world.example/v1/x'), 'https://world.example/v1/x')
  assert.equal(resolveShellUrl('wss://world.example/x'), 'wss://world.example/x')
})

shellCase('相对路径在壳里**响亮地失败**（壳里没有"页面来源"可解析）', () => {
  // 这是 web 版的同源形态：`/v1/...`。浏览器会对页面来源解析，壳里没有那个东西。
  assert.throws(() => resolveShellUrl('/v1/home'), /壳里没有"同源"可用/)
  assert.throws(() => resolveShellUrl('/home-status/status'), /壳里没有"同源"可用/)
  // 失败的文案要能指导用户（不是一句"出错了"）。
  assert.throws(() => resolveShellUrl('/v1/home'), /完整地址/)
})

shellCase('请求形状原样交给 Rust 侧：方法 / 头 / 体 / **取消信号**都不吞', async () => {
  const seen: Array<Record<string, unknown>> = []
  const controller = new AbortController()
  const transport = shellTransport(async (input, init) => {
    seen.push({ url: input, ...init })
    return new Response('{}', { status: 200 })
  })
  await transport.send({
    url: 'http://127.0.0.1:5190/status',
    method: 'PUT',
    headers: { Authorization: 'Bearer homekey' },
    body: '{"a":1}',
    signal: controller.signal,
  })
  assert.equal(seen.length, 1)
  assert.equal(seen[0]!['url'], 'http://127.0.0.1:5190/status')
  assert.equal(seen[0]!['method'], 'PUT')
  assert.deepEqual(seen[0]!['headers'], { Authorization: 'Bearer homekey' })
  assert.equal(seen[0]!['body'], '{"a":1}')
  assert.equal(seen[0]!['signal'], controller.signal, '取消信号必须传下去（家通道靠它超时）')
})

shellCase('响应只暴露那四个：status / ok / header / json / blob', async () => {
  const transport = shellTransport(async () =>
    new Response('{"n":7}', { status: 201, headers: { 'x-request-id': 'rid-9' } }),
  )
  const res = await transport.send({ url: 'http://x.test/v1/a', method: 'GET', headers: {} })
  assert.equal(res.status, 201)
  assert.equal(res.ok, true)
  assert.equal(res.header('x-request-id'), 'rid-9')
  assert.deepEqual(await res.json(), { n: 7 })

  const transport2 = shellTransport(async () => new Response('bytes', { status: 200 }))
  const res2 = await transport2.send({ url: 'http://x.test/v1/b', method: 'GET', headers: {} })
  assert.ok((await res2.blob()) instanceof Blob)
})

shellCase('相对路径**不会**被偷偷发出去（负对照：Rust 侧一次都不该被调用）', async () => {
  let called = 0
  const transport = shellTransport(async () => {
    called += 1
    return new Response('{}', { status: 200 })
  })
  await assert.rejects(
    () => transport.send({ url: '/v1/home', method: 'GET', headers: {} }),
    /壳里没有"同源"可用/,
  )
  assert.equal(called, 0, '解析失败时**绝不能**已经发出请求')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.equal(completed, DECLARED_CASES, `用例数 ${completed} 与声明的 ${DECLARED_CASES} 不符`)
  assert.ok(completed > 0, '本文件一条用例都没跑（空跑不算过）')
})
