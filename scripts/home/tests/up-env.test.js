'use strict'
/**
 * `up.js` 里**给宿主的进程环境**那两条判据（2026-10-09 真机实测换来的）。
 *
 * 为什么单独钉住：宿主必须**不经代理**够到本机世界与两个口 ⇒ `127.0.0.1` / `localhost` 一定要在
 * `NO_PROXY` 里；但从前那里是**直接覆盖**成常量 ⇒ 操作者自己加的绕过（例如给模型那家加一条）
 * 被静默吞掉 —— 他改环境变量也不生效，而症状是"她的模型请求读体中断 / 超时"（这台机器实测：
 * 宿主经代理 ⇒ 大回信被截断；CLI 探针走 node 的 `fetch`（默认不认代理）⇒ 直连成功 ⇒ 看着像
 * 宿主坏了）。⇒ 判据：**加两条，不是换一份**。
 */

const test = require('node:test')
const assert = require('node:assert/strict')

const { appendNoProxy, hostEnvironment } = require('../up')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

check('代理绕过：本机那两条一定在，操作者原有的取值一条不丢', () => {
  // 空 ⇒ 只补本机那两条。
  assert.equal(appendNoProxy(undefined), '127.0.0.1,localhost')
  assert.equal(appendNoProxy(''), '127.0.0.1,localhost')
  // 有 ⇒ **保留**，再补本机那两条（顺序：原有的在前）。
  assert.equal(appendNoProxy('example.com'), 'example.com,127.0.0.1,localhost')
  assert.equal(
    appendNoProxy('localhost,127.0.0.1,::1,opencode.ai'),
    'localhost,127.0.0.1,::1,opencode.ai',
    '已经有的不重复补、原有顺序不动',
  )
  // 脏输入（空格 / 空段）：**操作者那串原样保留**（只去空白与空段），本机那两条补一次。
  assert.equal(appendNoProxy(' a.com ,, b.com , a.com '), 'a.com,b.com,a.com,127.0.0.1,localhost')
})

check('宿主环境：大小写两份一致；模型那两条按文件在场与否决定', () => {
  const base = { PATH: 'x', NO_PROXY: 'opencode.ai', HTTP_PROXY: 'http://127.0.0.1:7897' }
  // 文件在场（家里配了模型）⇒ **一个模型 env 都不给**，代理变量照旧保留（宿主自己决定走不走）。
  const withFile = hostEnvironment(base, { backend: 'b', apiKey: 'k', modelFromFile: true })
  assert.equal(withFile.DEEPSEEK_API_KEY, undefined)
  assert.equal(withFile.CELIRA_MODEL_API_KEY, undefined)
  assert.equal(withFile.HTTP_PROXY, 'http://127.0.0.1:7897', '不摘代理：那是这台机器的环境')
  assert.equal(withFile.NO_PROXY, 'opencode.ai,127.0.0.1,localhost')
  assert.equal(withFile.no_proxy, withFile.NO_PROXY, '两份必须一致（有的库只看小写那份）')
  assert.equal(withFile.PATH, 'x', '别的变量一个不动')
  assert.equal(withFile.CELIRA_IM_BACKEND_AUTH, 'b')
  // 文件不在场（老形状）⇒ 那两条原样交回（与从前逐字相同）。
  const noFile = hostEnvironment(base, { backend: 'b', apiKey: 'k', modelFromFile: false })
  assert.equal(noFile.DEEPSEEK_API_KEY, 'k')
  assert.equal(noFile.NO_PROXY, 'opencode.ai,127.0.0.1,localhost')
})

check('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一份必须真的跑过用例')
})
