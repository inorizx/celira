'use strict'
// `world_witness`（第 1 步：给世界那一份加见证）：**停世界记一份摘要，起世界之前核一次**。
//
// 这一族要钉住的是**判据**，不是"能算 sha256"：
//  1. **窗口**：见证只在「上一次干净收场」↔「下一次开机之前」之间成立 —— 世界跑着的时候
//     任何一句话都会改它，那是正常的，**不该被报**；
//  2. **判不了 ≠ 没改过**：没有上一条见证 / 此刻读不出来，两档都必须与"没改过"分开
//     （与 `world.js` 的三态、`proc.ledgerPid` 的四态同规）；
//  3. **只说事实，不判性质**：报出「变了」**不等于**报出「被篡改了」—— 对拥有整机全权的
//     主体，本地见证不是封印（蓄意伪造可以连见证一起重写）；
//  4. **只追加 ＋ 链值**：删掉中间一条 ⇒ 报出断点（形状照她自己那本账）；
//  5. **fail-closed**：认不得的键 / 认不得的 reason / 不是 JSON ⇒ 明确拒绝，不猜、不降级；
//  6. **住处**：`<家>/logs/`，**不进环境根**（见证是这台机器上的编排事实，与 pid/日志同族）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const W = require('../world_witness')

let cases = 0
function wCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 造一个临时家（真的落文件）：`im/core.db` ＋ `im/media/attachments/`。 */
function makeHome({ db = 'SQLite format 3\0fake', attachments = { 'a.bin': [1, 2, 3] } } = {}) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `celira-witness-${process.pid}-`))
  fs.mkdirSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true })
  fs.writeFileSync(path.join(home, 'im', 'core.db'), Buffer.from(db, 'binary'))
  for (const [name, bytes] of Object.entries(attachments)) {
    fs.writeFileSync(path.join(home, 'im', 'media', 'attachments', name), Buffer.from(bytes))
  }
  return home
}
function drop(home) {
  fs.rmSync(home, { recursive: true, force: true })
}

wCase('★ 干净收场记一条：摘要是那份事实，链的起点是 null', () => {
  const home = makeHome()
  try {
    const snap = W.snapshot(home)
    assert.equal(snap.core.size, Buffer.from('SQLite format 3\0fake', 'binary').length)
    assert.equal(snap.core.sha256.length, 64)
    assert.deepEqual(snap.media, { count: 1, bytes: 3 })

    const out = W.recordCleanStop(home, { atMs: 1000 })
    assert.ok(out, '事实在场 ⇒ 记下了')
    assert.equal(out.record.prev, null, '第一条是链的起点')
    assert.equal(out.record.reason, 'clean-stop')
    assert.equal(out.record['core-sha256'], snap.core.sha256)

    const back = W.readRecords(home)
    assert.equal(back.length, 1)
    assert.deepEqual(back[0], out.record, '写下去再读回来逐字段相同')
  } finally {
    drop(home)
  }
})

wCase('★ 链：第二条的 prev 就是第一条的摘要；链条完整', () => {
  const home = makeHome()
  try {
    const a = W.recordCleanStop(home, { atMs: 1000 })
    const b = W.recordCleanStop(home, { atMs: 2000 })
    assert.equal(b.record.prev, W.digestRecord(a.record))
    const records = W.readRecords(home)
    assert.equal(records.length, 2)
    assert.equal(W.chainBreak(records), null, '链完整')
  } finally {
    drop(home)
  }
})

wCase('★ 删掉中间一条 ⇒ 报出断点（只追加那一半才成立）', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    W.recordCleanStop(home, { atMs: 2000 })
    W.recordCleanStop(home, { atMs: 3000 })
    const all = W.readRecords(home)
    assert.equal(all.length, 3)
    // 抽掉中间那条，重写成"看着连续"的两条
    fs.writeFileSync(W.witnessPath(home), W.encode(all[0]) + W.encode(all[2]))
    const records = W.readRecords(home)
    assert.equal(records.length, 2, '它仍然解析得出来')
    const brk = W.chainBreak(records)
    assert.ok(brk, '但链条断了 —— 这就是它存在的理由')
    assert.equal(brk.index, 1)
    assert.equal(brk.expected, W.digestRecord(all[0]))
    assert.equal(brk.found, all[2].prev)
  } finally {
    drop(home)
  }
})

wCase('★ 窗口：世界不在场时改库 ⇒ 报"变了"；再收场一次 ⇒ 新基线，报"相同"', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    assert.equal(W.checkBeforeStart(home).state, 'same', '什么都没动 ⇒ 相同')

    // 世界不在场的那段时间里，有人动过它
    fs.writeFileSync(path.join(home, 'im', 'core.db'), Buffer.from('SQLite format 3\0TAMPERED', 'binary'))
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'changed')
    assert.equal(r.detail, 'core')
    assert.match(r.text, /不一样/, '只说事实')
    assert.doesNotMatch(r.text, /篡改/, '**不判性质**')

    // 再干净收场一次 ⇒ 那成了新的基线
    W.recordCleanStop(home, { atMs: 2000 })
    assert.equal(W.checkBeforeStart(home).state, 'same')
  } finally {
    drop(home)
  }
})

wCase('★ 附件计数/字节变了也报"变了"（detail=media）', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    fs.writeFileSync(path.join(home, 'im', 'media', 'attachments', 'b.bin'), Buffer.from([9, 9]))
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'changed')
    assert.equal(r.detail, 'media')
  } finally {
    drop(home)
  }
})

wCase('★ 判不了 ≠ 没改过：没有上一条见证 ⇒ no-witness', () => {
  const home = makeHome()
  try {
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'no-witness')
    assert.equal(r.records, 0)
    assert.match(r.text, /判不了/)
    assert.doesNotMatch(r.text, /没改过$/, '不许把"判不了"说成"没改过"')
  } finally {
    drop(home)
  }
})

wCase('★ 世界不见了 ≠ 判不了：core.db 没了而见证说它还在 ⇒ changed（这是大事）', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    fs.rmSync(path.join(home, 'im', 'core.db'), { force: true })
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'changed', '有人把世界那一份删了 —— 不许读成"判不了"')
    assert.equal(r.detail, 'core-missing')
    assert.match(r.text, /不在了/)
  } finally {
    drop(home)
  }
})

wCase('★ 世界还没建过 ≠ 判不了：全新家（core.db 不存在、也没有见证）⇒ no-witness', () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `celira-witness-${process.pid}-`))
  try {
    assert.equal(fs.existsSync(path.join(home, 'im', 'core.db')), false)
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'no-witness', '第一次开机，世界还没建过')
    assert.equal(r.records, 0)
    assert.equal(W.recordCleanStop(home, { atMs: 1000 }), null, '还没有东西可记 ⇒ 不写半条')
  } finally {
    drop(home)
  }
})

wCase('★ 判不了 ≠ 没改过：core 在但读不出来 ⇒ unreadable', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    // 拿一个**目录**冒充 core.db：存在（所以不是"不见了"），但读不出来
    fs.rmSync(path.join(home, 'im', 'core.db'), { force: true })
    fs.mkdirSync(path.join(home, 'im', 'core.db'))
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'unreadable')
    assert.match(r.text, /判不了/)
    assert.doesNotMatch(r.text, /没改过$/, '不许把"判不了"说成"没改过"')
  } finally {
    drop(home)
  }
})

wCase('★ 整份附件被删掉 ⇒ 那是**改动**，不是"判不了"（缺省不等于零的另一面）', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    fs.rmSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true, force: true })
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'changed', '附件从 1 份变成 0 份 —— 那是事实不见了')
    assert.equal(r.detail, 'media')
  } finally {
    drop(home)
  }
})

wCase('★ 事实读不出来时不写半条（记不下就是记不下）', () => {
  const home = makeHome()
  try {
    fs.rmSync(path.join(home, 'im', 'core.db'), { force: true })
    assert.equal(W.recordCleanStop(home, { atMs: 1000 }), null)
    assert.equal(fs.existsSync(W.witnessPath(home)), false, '一个字节都没写')
  } finally {
    drop(home)
  }
})

wCase('★ fail-closed：认不得的键 / 认不得的 reason / 不是 JSON，三种都明确拒绝', () => {
  const good = W.makeRecord({
    atMs: 1000,
    core: { size: 3, sha256: 'a'.repeat(64) },
    media: { count: 0, bytes: 0 },
    prev: null,
  })
  assert.deepEqual(W.decode(W.encode(good).trim()), good, '正例：写下去读回来是同一份')

  const extra = JSON.stringify({ ...good, extra: 1 })
  assert.throws(() => W.decode(extra), /认不得的键/)

  const badReason = JSON.stringify({ ...good, reason: 'crash' })
  assert.throws(() => W.decode(badReason), /认不得的 reason/)

  assert.throws(() => W.decode('{not json'), /不是 JSON/)
  assert.throws(() => W.decode('[]'), /不是对象/)
  assert.throws(() => W.decode(JSON.stringify({ ...good, prev: 'short' })), /prev/)
})

wCase('★ 住处：见证在 <家>/logs/ 下，**不进环境根、不进 im/**', () => {
  const home = makeHome()
  try {
    const p = W.witnessPath(home)
    assert.equal(path.relative(home, p), path.join('logs', W.WITNESS_NAME))
    assert.ok(!p.startsWith(path.join(home, 'agents')), '不在任何一支的环境根里')
    assert.ok(!p.startsWith(path.join(home, 'im')), '不跟世界的事实混在一处')
  } finally {
    drop(home)
  }
})

wCase('★ 起世界之前核：只读 —— 它一个字节都不写', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000 })
    const before = fs.readFileSync(W.witnessPath(home))
    const r = W.checkBeforeStart(home)
    assert.equal(r.state, 'same')
    assert.deepEqual(fs.readFileSync(W.witnessPath(home)), before, '核这一下不改账本')
  } finally {
    drop(home)
  }
})

wCase('★ 缺省不等于零：附件目录**不存在** ⇒ 零（合法），**在但读不出来** ⇒ 判不了', () => {
  const home = makeHome()
  try {
    // 不存在 ⇒ "世界还没有过附件"，不是"读不出来"
    fs.rmSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true, force: true })
    assert.deepEqual(W.summarizeMedia(W.mediaPath(home)), { count: 0, bytes: 0 })
    W.recordCleanStop(home, { atMs: 1000 })
    assert.equal(W.checkBeforeStart(home).state, 'same', '没有附件是合法状态，不该报"判不了"')

    // 有人把整份附件删掉 ⇒ 那是**改动**（0 ← 1），要报出来
    fs.mkdirSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true })
    fs.writeFileSync(path.join(home, 'im', 'media', 'attachments', 'a.bin'), Buffer.from([1, 2, 3]))
    assert.equal(W.checkBeforeStart(home).state, 'changed', '附件从无到有也是变了')

    // 在、但读不出来（拿一个普通文件冒充目录）⇒ 判不了
    fs.rmSync(path.join(home, 'im', 'media', 'attachments'), { recursive: true, force: true })
    fs.writeFileSync(path.join(home, 'im', 'media', 'attachments'), 'not a dir')
    assert.equal(W.summarizeMedia(W.mediaPath(home)), null)
    assert.equal(W.checkBeforeStart(home).state, 'unreadable')
  } finally {
    drop(home)
  }
})

wCase('★ 两个家共用一份见证 ⇒ 当场显形（other-home），不是"变了"也不是"没改过"', () => {
  const home = makeHome()
  try {
    W.recordCleanStop(home, { atMs: 1000, homeId: 'home-A' })
    assert.equal(W.checkBeforeStart(home, { homeId: 'home-A' }).state, 'same', '同一个家 ⇒ 正常核')
    const r = W.checkBeforeStart(home, { homeId: 'home-B' })
    assert.equal(r.state, 'other-home', '这一条不是这个家的')
    assert.equal(r.detail, 'home-A', '要点名它记的是谁')
    assert.match(r.text, /判不了/)
    // 没给 homeId（老调用点 / 手工跑）⇒ 不假装判得了，退回原来的判据
    assert.equal(W.checkBeforeStart(home).state, 'same')
  } finally {
    drop(home)
  }
})

wCase('★ home 进记录、写读一致；报"变了"时带上字节差', () => {
  const home = makeHome()
  try {
    const out = W.recordCleanStop(home, { atMs: 1000, homeId: 'home-A' })
    assert.equal(out.record.home, 'home-A')
    assert.equal(W.readRecords(home)[0].home, 'home-A', '写下去读回来还是它')

    const before = fs.statSync(path.join(home, 'im', 'core.db')).size
    fs.appendFileSync(path.join(home, 'im', 'core.db'), Buffer.from('XXXXX'))
    const r = W.checkBeforeStart(home, { homeId: 'home-A' })
    assert.equal(r.state, 'changed')
    assert.equal(r.delta, 5, '5 个字节 —— 读数要能对得上')
    assert.match(r.text, /\+5 字节/)
    assert.equal(before + 5, fs.statSync(path.join(home, 'im', 'core.db')).size)
  } finally {
    drop(home)
  }
})

wCase('★ 强杀留下的库不干净 ⇒ 记不下基线（不许拿"可能"当基准）', () => {
  const home = makeHome()
  try {
    // 干净：记得下
    assert.equal(W.isCleanlyStopped(home), true)
    assert.ok(W.recordCleanStop(home, { atMs: 1000 }), '干净停机 ⇒ 记得下')

    // 强杀留下的日志兄弟（`home.js down` 走的就是强杀）⇒ 记不下
    fs.writeFileSync(path.join(home, 'im', 'core.db-journal'), 'junk')
    assert.equal(W.isCleanlyStopped(home), false)
    assert.equal(W.recordCleanStop(home, { atMs: 2000 }), null, '不干净 ⇒ 一个字节都不写')
    assert.equal(W.readRecords(home).length, 1, '账本还是那一条 —— 没有假基准')

    // 三种后缀都算（-wal / -shm / -journal，与 backup.js 逐字相同）
    fs.rmSync(path.join(home, 'im', 'core.db-journal'))
    for (const suffix of W.DB_SIDECARS) {
      fs.writeFileSync(path.join(home, 'im', `core.db${suffix}`), 'junk')
      assert.equal(W.isCleanlyStopped(home), false, `${suffix} 也算不干净`)
      fs.rmSync(path.join(home, 'im', `core.db${suffix}`))
    }
    assert.equal(W.isCleanlyStopped(home), true, '清掉之后又干净了')
  } finally {
    drop(home)
  }
})

test('★ 这一套用例真的跑了（空跑不算通过）', () => {
  assert.ok(cases >= 10, `跑到的用例数 ${cases} —— 少于 10 说明有整块没执行`)
})
