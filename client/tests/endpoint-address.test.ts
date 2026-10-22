// **接口地址那一道**的纯逻辑测试（2026-10-23：base 地址存下来 ⇒ 运行时 404）。
//
// 这一层要钉的不是"函数返回什么形状"，而是**它挡住的那一类错**：
//  1. 只填到服务根（`…/v1`、`…/v1/`、只到主机）⇒ **必须**判不通过，并且提示要指得出差哪一段；
//  2. 合法地址（`…/v1/chat/completions`、`…/v1/responses`）⇒ 不许误伤 —— 误伤比漏放更坏，
//     它会让一条本来能用的配置存不下去；
//  3. 协议与地址**两边对不上**（选了 responses 却填 chat 那段）⇒ 要单独一种提示；
//  4. 只看 pathname：query / hash 不参与判据，但**也不能被它们骗过**；
//  5. 判据要落在**保存之前**（源级）—— 一个没人调用的纯函数挡不住任何 404。
//
// 运行： node --test --experimental-strip-types tests/endpoint-address.test.ts
//       （或 `npm test` 跑全量）

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { validateEndpointAddress } from '../src/api/endpoint.ts'
import { PROVIDER_PRESETS } from '../src/api/presets.ts'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/** 判"不通过"，并交出那句提示（好让下面逐个点它该说什么）。 */
function refusal(endpoint: string, protocol?: string): string {
  const result = validateEndpointAddress(endpoint, protocol)
  assert.equal(result.ok, false, `这个地址不该放行：${endpoint || '(空)'}`)
  return result.ok ? '' : result.message
}

test('★ 只填到服务根 ⇒ 不通过，而且提示点得出"差最后一段"', () => {
  // 真实案例：opencode-go 的正确地址是 …/zen/go/v1/chat/completions，填 base 会 404。
  for (const base of [
    'https://opencode.ai/zen/go/v1',
    'https://opencode.ai/zen/go/v1/',
    'https://api.deepseek.com/v1',
    'https://api.deepseek.com/v1/',
    'https://open.bigmodel.cn/api/paas/v4',
  ]) {
    const message = refusal(base)
    assert.match(message, /服务根|最后一段/, `${base} 的提示没说出差哪一段：${message}`)
    assert.match(message, /chat\/completions/, `${base} 的提示要点出该填到哪儿：${message}`)
  }
})

test('★ 只到主机 / 空 / 只有空白 ⇒ 不通过（各说各的话，不混成一句"错了"）', () => {
  assert.match(refusal('https://api.deepseek.com'), /只填到了主机/)
  assert.match(refusal('https://api.deepseek.com/'), /只填到了主机/)
  assert.match(refusal(''), /空着/)
  assert.match(refusal('   \n\t '), /空着/)
})

test('★ 合法地址一律放行：`…/v1/chat/completions` 与 `…/v1/responses`', () => {
  // 默认那一档（不给 protocol 就是 chat-completions）。
  assert.deepEqual(validateEndpointAddress('https://api.deepseek.com/v1/chat/completions'), {
    ok: true,
  })
  assert.deepEqual(validateEndpointAddress('https://api.deepseek.com/v1/chat/completions', ''), {
    ok: true,
  })
  assert.deepEqual(
    validateEndpointAddress('https://api.deepseek.com/v1/chat/completions', 'chat-completions'),
    { ok: true },
  )
  assert.deepEqual(
    validateEndpointAddress('https://api.openai.com/v1/responses', 'responses'),
    { ok: true },
  )
  // 首尾空白不算差别（与 `presetIdForEndpoint` 同一口径）。
  assert.deepEqual(
    validateEndpointAddress('  https://api.deepseek.com/v1/chat/completions  '),
    { ok: true },
  )
  // 末尾多一个斜杠也不该被误伤（见 `endpoint.ts` 文件头那条 ⚠）。
  assert.deepEqual(
    validateEndpointAddress('https://api.deepseek.com/v1/chat/completions/'),
    { ok: true },
  )
})

test('★ 非 http(s) / 读不成网址 ⇒ 不通过（协议只认两种）', () => {
  for (const bad of [
    'ftp://api.deepseek.com/v1/chat/completions',
    'file:///etc/passwd',
    'wss://api.deepseek.com/v1/chat/completions',
  ]) {
    assert.match(refusal(bad), /http:\/\/ 或 https:\/\//, `${bad} 的提示没说清协议`)
  }
  // 手填时最常见的漏写：没有协议头（`new URL` 直接抛）。
  assert.match(refusal('api.deepseek.com/v1/chat/completions'), /读不成网址/)
  assert.match(refusal('随便写点什么'), /读不成网址/)
})

test('★ 只看 pathname：query / hash 不影响判据，也骗不过判据', () => {
  assert.deepEqual(
    validateEndpointAddress('https://gw.example/v1/chat/completions?api-version=2024-10-21#x'),
    { ok: true },
  )
  assert.deepEqual(validateEndpointAddress('https://gw.example/v1/responses?beta=1', 'responses'), {
    ok: true,
  })
  // ★ 判据只看路径：把"该有的那一段"藏在 query 里 **不许**算数。
  const message = refusal('https://gw.example/v1?next=/v1/chat/completions')
  assert.match(message, /服务根|最后一段/, `query 里的那一段被当成了地址：${message}`)
})

test('★ 协议与地址两边对不上 ⇒ 单独一种提示（说的不是"地址不完整"）', () => {
  const asResponses = refusal('https://api.openai.com/v1/chat/completions', 'responses')
  assert.match(asResponses, /接口类型选的是 responses/, asResponses)
  assert.match(asResponses, /\.\.\.\/responses|\/responses/, '要指出该填哪一段：' + asResponses)

  const asChat = refusal('https://api.openai.com/v1/responses')
  assert.match(asChat, /接口类型选的是 chat-completions/, asChat)
  assert.match(asChat, /\/chat\/completions/, '要指出该填哪一段：' + asChat)

  // 老式 `/v1/completions` 不是这一层认的那一段（agent 走的是 chat-completions）。
  assert.equal(validateEndpointAddress('https://api.deepseek.com/v1/completions').ok, false)
})

test('★ 预设表里每一条地址都过这一道（表与判据不许各说各的）', () => {
  for (const preset of PROVIDER_PRESETS) {
    const result = validateEndpointAddress(preset.endpointTemplate)
    assert.ok(
      result.ok,
      `预设「${preset.id}」的地址过不了保存前这道校验 ⇒ 用户选它就存不下去：` +
        `${preset.endpointTemplate}（${result.ok ? '' : result.message}）`,
    )
  }
})

test('★ OpenCode Go 是完整地址（base 会 404 的那一家）', () => {
  const go = PROVIDER_PRESETS.find((preset) => preset.name === 'OpenCode Go')
  assert.ok(go, '预设表里没有 OpenCode Go')
  assert.equal(go?.endpointTemplate, 'https://opencode.ai/zen/go/v1/chat/completions')
  assert.deepEqual(validateEndpointAddress(go!.endpointTemplate), { ok: true })
})

test('★ 判据落在**保存之前**（源级）：校验不过就一个请求都不发', () => {
  // ⚠ 为什么是源级：本仓 client 测试是 `node --test` + 类型剥离，**没有 jsdom / 组件渲染**
  //   （与 `presets.test.ts` 的 `PRESET_DISCLAIMER` 那条同款处境）。能离线钉住的是
  //   "保存那条路上先调它、且在发请求之前"；真机上的那一屏由手动验收看。
  const page = read('../src/pages/ModelProvidersPage.tsx')
  const start = page.indexOf('const saveDraft')
  const end = page.indexOf('const usersOfEntry')
  assert.ok(start > 0 && end > start, '`saveDraft` 的形状变了（找不到它的体）')
  const body = page.slice(start, end)

  const checkAt = body.indexOf('validateEndpointAddress(')
  assert.ok(checkAt > -1, '`saveDraft` 里没有这道校验 —— 纯函数没人调用就挡不住任何 404')

  for (const request of ['await run(', 'createHomeSource(', 'updateHomeSource(']) {
    const at = body.indexOf(request)
    assert.ok(at > -1, `saveDraft 里找不到「${request}」（判据的锚点变了）`)
    assert.ok(
      checkAt < at,
      `校验必须排在「${request}」之前 —— 否则地址不完整时请求已经发出去了`,
    )
  }

  // 不通过时要把那句人话摆到**现有的失败位**（不新造一套 UI）。
  assert.match(body, /setActionError\(\s*addressed\.message\s*\)/, '提示没有接到 actionError 那一格')
})
