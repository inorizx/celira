'use strict'
// `model_witness`：**模型配置那两份文件的见证**（谁最后改过这一格）。
//
// 这一族要钉住的是**判据**，不是"能算 sha256"：
//  1. **两档分开**：门改的**有时刻**；别处改的**只知变了** —— 而"没有见证"与"没改过"
//     必须分开（判不了 ≠ 没改过，与 `world_witness.js` / `world.js` 的三态同规）；
//  2. **缝在门上**：账本只由**门**记；`writeAtomic` 那条路（迁移、布置）不记 —— 记了就会
//     把"布置"说成"页面改的"（那一句假话正是这一刀要防的）；
//  3. **"清"也算一笔写**：摘掉绑定要留一条（`sha256: null`），否则页面上的"解除指定"
//     会在读数里变成"别处有人动过它"；
//  4. **只追加 ＋ 链值**：删掉中间一条 ⇒ 报出断点；链值算在**一条规范行**上
//     （两个写者两种语言，各自的 JSON 转义不是同一件事）；
//  5. **fail-closed**：认不得的键 / 非 ASCII 的"哪一份" / 坏摘要 ⇒ 明确拒绝，不猜、不降级；
//  6. **记不下不许让那一笔写失败**：`recordWrite` 回 `{ok:false}`，**绝不抛**。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const W = require('../model_witness')

let cases = 0
function wCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/**
 * **跨语言向量**（与壳那一份 `app/src-tauri/src/model_witness.rs` 的用例**钉同一个十六进制**）。
 *
 * 两边各钉一份、值必须相同：改任何一边的规范行，两处里必有一处红。
 */
const VECTOR = {
  first: '28d6c89ea3d3f0ddfafc178d675b1de2d453f4255cc6ee37f7f2d1ca0278208f',
  second: '9b48432c475ee76105f35dcb4ca436c1456a71c4edbbe9b72ddeba70fd66e496',
}

/** 造一个临时家：清单 ＋ 一支住户的绑定。 */
function makeHome({ list = '[{"id":"ms-1"}]\n', binding = '{\n  "source": "ms-1"\n}\n' } = {}) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `celira-model-witness-${process.pid}-`))
  if (list !== null) fs.writeFileSync(path.join(home, 'model_sources.json'), list)
  if (binding !== null) {
    fs.mkdirSync(path.join(home, 'agents', 'a-1'), { recursive: true })
    fs.writeFileSync(path.join(home, 'agents', 'a-1', 'model_binding.json'), binding)
  }
  return home
}
function drop(home) {
  fs.rmSync(home, { recursive: true, force: true })
}
const listOf = (home) => path.join(home, 'model_sources.json')
const bindingOf = (home) => path.join(home, 'agents', 'a-1', 'model_binding.json')

// ---------------------------------------------------------------------------
// 一、形状：规范行 ＋ 链值（跨语言那一把锁）
// ---------------------------------------------------------------------------

wCase('★ 规范行与链值：两个写者两种语言，算在**一条显式规范行**上', () => {
  const first = {
    'at-ms': 1760000000000,
    file: 'model_sources.json',
    size: 412,
    sha256: 'a'.repeat(64),
    prev: null,
  }
  assert.equal(
    W.canonicalLine(first),
    `1760000000000\nmodel_sources.json\n412\n${'a'.repeat(64)}\n\n`,
    '规范行的形状逐字钉死：<at-ms>\\n<file>\\n<size>\\n<sha256|空>\\n<prev|空>\\n',
  )
  assert.equal(W.digestRecord(first), VECTOR.first)
  // 第二条：`prev` 接上第一条 ⇒ 链把历史绑在一起；`sha256: null`（摘掉）在规范行里是**空**。
  const second = {
    'at-ms': 1760000000001,
    file: 'agents/a-1/model_binding.json',
    size: 0,
    sha256: null,
    prev: VECTOR.first,
  }
  assert.equal(W.digestRecord(second), VECTOR.second)
})

wCase('★ 认不得的形状一律拒绝（fail-closed，不猜、不降级）', () => {
  // ⚠ `makeRecord` 收的是**驼峰那一格**（`atMs`），落盘那一格才是 `at-ms`
  //   —— 与 `world_witness.js` 的 `makeRecord` 同一个形状。
  const good = { 'at-ms': 1, file: 'model_sources.json', size: 0, sha256: null, prev: null }
  assert.deepEqual(W.makeRecord({ atMs: 1, file: 'model_sources.json', size: 0, sha256: null, prev: null }), good)
  // 认不得的键（多一个、少一个都算）
  assert.throws(() => W.decode(JSON.stringify({ ...good, extra: 1 })), /认不得的键/)
  const { size, ...missing } = good
  void size
  assert.throws(() => W.decode(JSON.stringify(missing)), /认不得的键/)
  assert.throws(() => W.decode('不是 JSON'), /不是 JSON/)
  assert.throws(() => W.decode('[1,2]'), /不是对象/)
  // `file` 要可打印 ASCII（两个写者要逐字节对得上）
  const shape = { atMs: 1, file: 'model_sources.json', size: 0, sha256: null, prev: null }
  assert.throws(() => W.makeRecord({ ...shape, file: 'agents/她/model_binding.json' }), /ASCII/)
  assert.throws(() => W.makeRecord({ ...shape, file: '' }), /ASCII/)
  // 摘要要么 64 位十六进制、要么 null（"那一刻它不在"）
  assert.throws(() => W.makeRecord({ ...shape, sha256: 'xyz' }), /sha256/)
  assert.throws(() => W.makeRecord({ ...shape, sha256: 'A'.repeat(64) }), /sha256/)
  assert.throws(() => W.makeRecord({ ...shape, prev: 'short' }), /prev/)
  assert.throws(() => W.makeRecord({ ...shape, atMs: -1 }), /at-ms/)
})

wCase('★ 「哪一份」是**相对家的路径**：家外 / 非 ASCII ⇒ 说不成（不猜一个名字）', () => {
  const home = makeHome()
  try {
    assert.equal(W.relativeFile(home, listOf(home)), 'model_sources.json')
    assert.equal(W.relativeFile(home, bindingOf(home)), 'agents/a-1/model_binding.json')
    assert.equal(W.relativeFile(home, path.join(path.dirname(home), 'x.json')), null, '家外 ⇒ null')
    assert.equal(W.relativeFile(home, path.join(home, 'agents', '她.json')), null, '非 ASCII ⇒ null')
  } finally {
    drop(home)
  }
})

// ---------------------------------------------------------------------------
// 二、判：四态（判不了必须与"没改过"分开）
// ---------------------------------------------------------------------------

wCase('★ 门写过之后没再变过 ⇒ same（**有时刻**）', () => {
  const home = makeHome()
  try {
    const out = W.recordWrite(home, listOf(home), 1000)
    assert.equal(out.ok, true)
    const reading = W.readingFor(W.open(home), home, listOf(home))
    assert.deepEqual(reading, { state: 'same', atMs: 1000, detail: null, deltaBytes: null })
  } finally {
    drop(home)
  }
})

wCase('★★ 别处改的（她走 terminal 写）⇒ changed，而"是谁"这条路上答不出来', () => {
  const home = makeHome()
  try {
    assert.equal(W.recordWrite(home, listOf(home), 1000).ok, true)
    // 门外的那只手：直接改文件（她走 terminal、或别的工具 —— 在这条路上**同一个形状**）。
    fs.appendFileSync(listOf(home), ' ')
    const reading = W.readingFor(W.open(home), home, listOf(home))
    assert.equal(reading.state, 'changed')
    assert.equal(reading.detail, 'content')
    assert.equal(reading.atMs, 1000, '知道的是**门最后写于何时**，不是"谁改的"')
    assert.equal(reading.deltaBytes, 1, '字节差要对得上')
  } finally {
    drop(home)
  }
})

wCase('★ 门写过它、现在它不在了 ⇒ changed/missing；门摘掉过、现在还在 ⇒ changed/appeared', () => {
  const home = makeHome()
  try {
    assert.equal(W.recordWrite(home, bindingOf(home), 1000).ok, true)
    fs.rmSync(bindingOf(home))
    assert.deepEqual(W.readingFor(W.open(home), home, bindingOf(home)), {
      state: 'changed',
      atMs: 1000,
      detail: 'missing',
      deltaBytes: null,
    })
    // 门自己摘的 ⇒ 记一条"清"：那一刻它不在，而此刻也不在 ⇒ **一致**（不是"有人动过"）。
    assert.equal(W.recordClear(home, bindingOf(home), 2000).ok, true)
    assert.deepEqual(W.readingFor(W.open(home), home, bindingOf(home)), {
      state: 'same',
      atMs: 2000,
      detail: null,
      deltaBytes: null,
    })
    // 有人把它放回来了
    fs.writeFileSync(bindingOf(home), '{\n  "source": "ms-1"\n}\n')
    assert.deepEqual(W.readingFor(W.open(home), home, bindingOf(home)), {
      state: 'changed',
      atMs: 2000,
      detail: 'appeared',
      deltaBytes: null,
    })
  } finally {
    drop(home)
  }
})

wCase('★ 判不了的两档必须分开：没有见证 / 这一份读不出来 / 这本账读不出来', () => {
  const home = makeHome()
  try {
    // ① 从没被门碰过 ⇒ no-witness（**不是**"没改过"）
    assert.deepEqual(W.readingFor(W.open(home), home, listOf(home)), {
      state: 'no-witness',
      atMs: null,
      detail: null,
      deltaBytes: null,
    })
    // ② 这一份在、但读不出来 ⇒ unreadable/file
    fs.rmSync(listOf(home))
    fs.mkdirSync(listOf(home))
    assert.deepEqual(W.readingFor(W.open(home), home, listOf(home)), {
      state: 'unreadable',
      atMs: null,
      detail: 'file',
      deltaBytes: null,
    })
    fs.rmdirSync(listOf(home))
    fs.writeFileSync(listOf(home), '[{"id":"ms-1"}]\n')
    // ③ 账本里有一行读不出来 ⇒ unreadable/ledger（整本判不了，**不是**"没改过"）
    assert.equal(W.recordWrite(home, listOf(home), 1000).ok, true)
    fs.appendFileSync(W.witnessPath(home), '{半行\n')
    const ledger = W.open(home)
    assert.match(ledger.broken, /不是 JSON/)
    assert.deepEqual(W.readingFor(ledger, home, listOf(home)), {
      state: 'unreadable',
      atMs: null,
      detail: 'ledger',
      deltaBytes: null,
    })
  } finally {
    drop(home)
  }
})

// ---------------------------------------------------------------------------
// 三、账本：只追加 ＋ 链值 ＋ 按份看
// ---------------------------------------------------------------------------

wCase('★ 账本按**份**看：两份文件各记各的，互不冒充', () => {
  const home = makeHome()
  try {
    assert.equal(W.recordWrite(home, listOf(home), 1000).ok, true)
    assert.equal(W.recordWrite(home, bindingOf(home), 1001).ok, true)
    const ledger = W.open(home)
    assert.equal(ledger.records.length, 2)
    assert.equal(ledger.records[1].prev, W.digestRecord(ledger.records[0]), '链接上了')
    assert.equal(ledger.last.get('model_sources.json')['at-ms'], 1000)
    assert.equal(ledger.last.get('agents/a-1/model_binding.json')['at-ms'], 1001)
    // 只动一份 ⇒ 另一份照旧 same（**不许**因为账里"有别人动过"就一起报红）
    fs.appendFileSync(listOf(home), ' ')
    assert.equal(W.readingFor(ledger, home, bindingOf(home)).state, 'same')
  } finally {
    drop(home)
  }
})

wCase('★ 删掉中间一条 ⇒ 报出断点（只追加的账，断点就是证据）', () => {
  const home = makeHome()
  try {
    for (const at of [1000, 1001, 1002]) assert.equal(W.recordWrite(home, listOf(home), at).ok, true)
    assert.equal(W.open(home).chain, null, '三条连着 ⇒ 链完整')
    const all = W.readRecords(home)
    fs.writeFileSync(W.witnessPath(home), W.encode(all[0]) + W.encode(all[2]))
    const ledger = W.open(home)
    assert.equal(ledger.chain.index, 1, '断点在第 2 条（0 起算的第 1 条）')
    assert.equal(ledger.chain.expected, W.digestRecord(all[0]))
  } finally {
    drop(home)
  }
})

wCase('★ 记不下**绝不抛**：说不成"哪一份" ⇒ 回一个 reason，那一笔写照旧成立', () => {
  const home = makeHome()
  try {
    const outside = path.join(path.dirname(home), 'outside.json')
    fs.writeFileSync(outside, '{}')
    const out = W.recordWrite(home, outside, 1000)
    assert.equal(out.ok, false)
    assert.equal(out.reason, 'unnamed')
    assert.equal(fs.existsSync(W.witnessPath(home)), false, '一个字节都没写')
    fs.rmSync(outside)
    // 这一份读不出来（在、但读不动）⇒ 也回 reason，不抛
    fs.rmSync(listOf(home))
    fs.mkdirSync(listOf(home))
    assert.deepEqual(W.recordWrite(home, listOf(home), 1000), { ok: false, reason: 'unreadable' })
  } finally {
    drop(home)
  }
})

wCase('★ 住处：`<家>/logs/model-witness.ndjson`（**不进环境根** —— 它不跟家搬）', () => {
  const home = makeHome()
  try {
    assert.equal(W.witnessPath(home), path.join(home, 'logs', 'model-witness.ndjson'))
    assert.equal(W.recordWrite(home, listOf(home), 1000).ok, true)
    const raw = fs.readFileSync(W.witnessPath(home), 'utf8')
    assert.equal(raw.split('\n').filter((line) => line !== '').length, 1)
    assert.deepEqual(W.decode(raw.trimEnd()), W.readRecords(home)[0], '写下去读回来逐字相同')
    assert.equal(fs.existsSync(path.join(home, 'agents', 'a-1', 'logs')), false, '环境根里没有它')
  } finally {
    drop(home)
  }
})

wCase('★ 凭据一个字节都不进账本（清单里那把钥匙不是这一层的事）', () => {
  const secret = 'sk-do-not-log-me'
  const home = makeHome({ list: `[{"id":"ms-1","api_key":"${secret}"}]\n` })
  try {
    assert.equal(W.recordWrite(home, listOf(home), 1000).ok, true)
    const raw = fs.readFileSync(W.witnessPath(home), 'utf8')
    assert.equal(raw.includes(secret), false, '账里只有摘要，没有内容')
    assert.equal(raw.includes('api_key'), false)
    // 摘要算在整份文件上 ⇒ 内容变了摘要就变（而它**回不出来**内容）
    fs.writeFileSync(listOf(home), `[{"id":"ms-1","api_key":"${secret}x"}]\n`)
    assert.equal(W.readingFor(W.open(home), home, listOf(home)).state, 'changed')
  } finally {
    drop(home)
  }
})

// ---------------------------------------------------------------------------
// 四、与壳那一份对表（两个写者、一种格式）
// ---------------------------------------------------------------------------

wCase('★ 与壳那一份对表：文件名、键名、跨语言向量三样都钉住', () => {
  const shell = fs.readFileSync(
    path.join(__dirname, '..', '..', '..', 'app', 'src-tauri', 'src', 'model_witness.rs'),
    'utf8',
  )
  assert.match(shell, /pub const WITNESS_NAME: &str = "model-witness\.ndjson";/)
  assert.match(
    shell,
    /pub const RECORD_KEYS: \[&str; 5\] = \["at-ms", "file", "prev", "sha256", "size"\];/,
    '键名与顺序逐字相同（JS 那份也按这个顺序排过）',
  )
  assert.ok(
    shell.includes(VECTOR.first) && shell.includes(VECTOR.second),
    '壳那一份的用例钉的是**同一个向量** —— 改任何一边的规范行，必有一处红',
  )
})

test('★ 这一套用例真的跑了（空跑不算通过）', () => {
  assert.ok(cases >= 10, `跑到的用例数 ${cases} —— 少于 10 说明有整块没执行`)
})
