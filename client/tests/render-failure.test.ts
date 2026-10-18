// 渲染失败分类与降级文案的纯逻辑测试。
//
// 运行：cd client && npm test
// 覆盖：各家浏览器/打包器"动态模块没拿到"的原话 ⇒ `chunk-load`；其余一律 `render`。
//       ★ 本文件最要紧的是**负向**那两条：普通的网络失败原话**不许**被判成 chunk-load ——
//       把真正的代码 bug 显示成"页面已更新，重新加载一下"就是教用户白费力气。
//       另钉：两类文案彼此不同、且不含机器话。

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CHUNK_FAILURE_TITLE,
  RELOAD_LABEL,
  RENDER_FAILURE_TITLE,
  classifyRenderFailure,
  renderFailureCopy,
} from '../src/lib/renderFailure.ts'

/** 门禁不许空跑（与 `front-door.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
// 总数**含最后那条**（与 `front-door.test.ts` 同口径）：它执行时前 12 条已经跑完。
const DECLARED_CASES = 13

function failureCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

// ---------------------------------------------------------------------------
// 认得出：四家措辞 + 打包器对象形状
// ---------------------------------------------------------------------------

failureCase('Chromium 的措辞 ⇒ chunk-load', () => {
  // 原话形状（实测自 Chromium）：后面跟的是那个取不到的 URL。
  assert.equal(
    classifyRenderFailure(
      new TypeError(
        'Failed to fetch dynamically imported module: http://localhost:5173/assets/ContactsPage-BwsZdUvL.js',
      ),
    ),
    'chunk-load',
  )
})

failureCase('Firefox 的措辞 ⇒ chunk-load', () => {
  assert.equal(
    classifyRenderFailure(new Error('error loading dynamically imported module')),
    'chunk-load',
  )
})

failureCase('WebKit 的措辞 ⇒ chunk-load', () => {
  assert.equal(
    classifyRenderFailure(new Error('Importing a module script failed.')),
    'chunk-load',
  )
})

failureCase('打包器措辞（含只报类名的那种）⇒ chunk-load', () => {
  assert.equal(classifyRenderFailure(new Error('Loading chunk 42 failed.')), 'chunk-load')
  assert.equal(
    classifyRenderFailure(new Error('ChunkLoadError: Loading chunk 7 failed.')),
    'chunk-load',
  )
})

failureCase('抛的是普通对象（鸭子类型）⇒ 也认得出', () => {
  assert.equal(
    classifyRenderFailure({ name: 'ChunkLoadError', message: 'loading chunk vendors failed' }),
    'chunk-load',
  )
})

failureCase('大小写不影响判定', () => {
  assert.equal(
    classifyRenderFailure(new Error('FAILED TO FETCH DYNAMICALLY IMPORTED MODULE')),
    'chunk-load',
  )
})

// ---------------------------------------------------------------------------
// ★ 负向：认不出的必须落到 render（fail-closed 的方向）
// ---------------------------------------------------------------------------

failureCase('★ 普通的 fetch 失败原话**不许**被判成 chunk-load', () => {
  // 这两句是断网/接口挂了时 fetch 的常见原话，它们是**另一件事实**：
  // 文件都在，只是请求没成功。判成 chunk-load 会让页面说"已更新，重新加载一下就好"，
  // 而重新加载根本不解决问题 —— 那是把用户引去白费力气。
  assert.equal(classifyRenderFailure(new TypeError('Failed to fetch')), 'render')
  assert.equal(
    classifyRenderFailure(new TypeError('NetworkError when attempting to fetch resource.')),
    'render',
  )
  // 顺带钉住：认得出与认不出之间只差"dynamically imported module"这几个词。
  assert.equal(classifyRenderFailure(new Error('Failed to fetch module.json')), 'render')
})

failureCase('★ 不是错误的东西一律落 render，绝不抛错', () => {
  for (const value of [undefined, null, 0, false, {}, [], new Error('')]) {
    assert.equal(classifyRenderFailure(value), 'render', `value=${JSON.stringify(value)}`)
  }
  // 空对象没有 name/message ⇒ 搜出来是空串 ⇒ 不能撞上任何模式。
  assert.equal(classifyRenderFailure(Object.create(null)), 'render')
})

// ---------------------------------------------------------------------------
// 文案：两类必须分得开，且是"人话"
// ---------------------------------------------------------------------------

failureCase('两类标题不同、正文不同，都不为空', () => {
  const chunk = renderFailureCopy('chunk-load')
  const render = renderFailureCopy('render')
  assert.notEqual(chunk.title, render.title)
  assert.notEqual(chunk.body, render.body)
  for (const copy of [chunk, render]) {
    assert.ok(copy.title.trim().length > 0, '标题不能空')
    assert.ok(copy.body.trim().length > 0, '正文不能空')
  }
  assert.equal(chunk.title, CHUNK_FAILURE_TITLE)
  assert.equal(render.title, RENDER_FAILURE_TITLE)
})

failureCase('chunk-load 不给「返回消息」这条路（别的页面的 chunk 同样可能没拿到）', () => {
  // 这是本仓既有纪律的落点：**不给点了必然报错的按钮**。
  // chunk 拿不到时，"返回消息"那一页的 chunk 往往也没拿到 ⇒ 只留「重新加载」。
  assert.equal(renderFailureCopy('chunk-load').canReturnToMessages, false)
  assert.equal(renderFailureCopy('render').canReturnToMessages, true)
})

failureCase('文案里不许出现机器话（分类码 / HTTP 状态码形状）', () => {
  const all = [
    renderFailureCopy('chunk-load').title,
    renderFailureCopy('chunk-load').body,
    renderFailureCopy('render').title,
    renderFailureCopy('render').body,
    RELOAD_LABEL,
  ].join('\n')
  assert.ok(!/_/.test(all), `文案里出现了下划线形态的分类码：${all}`)
  assert.ok(!/HTTP\s*\d/.test(all), `文案里出现了 HTTP 状态码：${all}`)
})

failureCase('「重新加载」这颗按钮的文案在场（真机门按它判降级页）', () => {
  assert.equal(RELOAD_LABEL, '重新加载')
})

// ---------------------------------------------------------------------------
// 防空跑
// ---------------------------------------------------------------------------

failureCase('门禁不许空跑：本文件真的执行了用例', () => {
  // 减一：本条自己也是用例。
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
