'use strict'
/**
 * 搜索源的**清单 + 绑定**：CLI 写侧的行为（2026-10-20）。
 *
 * 判据四件：
 * 1. `add` 写的形状与读侧（`agent/src/web_source.rs`）逐键对齐；
 * 2. `use` 把绑定写进**那一支**的环境根；
 * 3. **被绑定的条目不许删**（删了绑定就悬空，读侧会拒绝）；
 * 4. key **永不打印**（列表只印掩码）。
 */

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const test = require('node:test')

const S = require('../web-sources')

/** 一个临时家（带一支住户）。 */
function home(label) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-web-${label}-`))
  fs.mkdirSync(path.join(root, 'agents', 'a-1'), { recursive: true })
  fs.writeFileSync(path.join(root, 'agents', 'a-1', 'identity.json'), '{}')
  return root
}

/** 跑一条子命令（把它的退出码带回来；stdout 会一起打出来，不去劫持它）。 */
function run(homeDir, argv) {
  return S.cli([...argv, '--home', homeDir])
}

test('add 写出的形状与读侧逐键对齐（方言决定要不要模型名）', () => {
  const dir = home('add')
  assert.equal(run(dir, ['add', '--provider', 'deepseek', '--key', 'sk-sentinel']), 0)
  assert.equal(run(dir, ['add', '--provider', 'tavily', '--key', 'tvly-sentinel', '--name', '备用']), 0)
  const list = JSON.parse(fs.readFileSync(S.listPath(dir), 'utf8'))
  assert.equal(list.length, 2)
  assert.deepEqual(
    { id: list[0].id, search: list[0].search.wire, fetch: list[0].fetch.wire },
    {
      id: 'ss-1',
      search: S.PROVIDERS.deepseek.search.wire,
      fetch: S.PROVIDERS.deepseek.fetch.wire,
    },
  )
  assert.equal(list[0].search.endpoint, S.PROVIDERS.deepseek.search.endpoint)
  assert.equal(list[0].fetch.endpoint, S.PROVIDERS.deepseek.fetch.endpoint)
  assert.equal(list[1].id, 'ss-2')
  assert.equal(list[1].search.wire, 'results-json')
  assert.equal(list[1].fetch.wire, 'fetch-raw')
  fs.rmSync(dir, { recursive: true, force: true })
})

test('use 把绑定写进那一支的环境根；不认识的 id 被拒', () => {
  const dir = home('use')
  run(dir, ['add', '--provider', 'deepseek', '--key', 'sk-sentinel'])
  assert.equal(run(dir, ['use', 'ss-1']), 0)
  const binding = JSON.parse(
    fs.readFileSync(path.join(dir, 'agents', 'a-1', S.BINDING_FILE), 'utf8'),
  )
  assert.deepEqual(binding, { source: 'ss-1' })
  assert.equal(run(dir, ['use', 'ss-9']), 1, '清单里没有的 id 不许绑')
  fs.rmSync(dir, { recursive: true, force: true })
})

test('被绑定的条目不许删；解绑之后才删得掉', () => {
  const dir = home('remove')
  run(dir, ['add', '--provider', 'deepseek', '--key', 'sk-sentinel'])
  run(dir, ['use', 'ss-1'])
  assert.equal(run(dir, ['remove', 'ss-1']), 1, '悬空绑定比删不掉更坏')
  fs.rmSync(path.join(dir, 'agents', 'a-1', S.BINDING_FILE))
  assert.equal(run(dir, ['remove', 'ss-1']), 0)
  assert.deepEqual(JSON.parse(fs.readFileSync(S.listPath(dir), 'utf8')), [])
  fs.rmSync(dir, { recursive: true, force: true })
})

test('空家是「一条都没有」，不是错误', () => {
  const dir = home('empty')
  assert.equal(run(dir, ['list']), 0)
  assert.equal(run(dir, ['show']), 0, '没绑定 ⇒ 走环境变量那一条老路')
  fs.rmSync(dir, { recursive: true, force: true })
})

test('掩码只露最后四位（key 永不打印）', () => {
  assert.equal(S.maskKey('sk-1234567890abcd'), '••••abcd')
  assert.equal(S.maskKey('短'), '••••')
  assert.ok(!S.maskKey('sk-1234567890abcd').includes('1234567890'))
})

test('清单读坏了：list 报错，add 不覆盖它', () => {
  const dir = home('broken')
  fs.writeFileSync(S.listPath(dir), '{ 这不是清单')
  assert.equal(run(dir, ['list']), 1)
  assert.equal(run(dir, ['add', '--provider', 'deepseek', '--key', 'k']), 1)
  assert.equal(fs.readFileSync(S.listPath(dir), 'utf8'), '{ 这不是清单', '坏文件原样保留')
  fs.rmSync(dir, { recursive: true, force: true })
})
