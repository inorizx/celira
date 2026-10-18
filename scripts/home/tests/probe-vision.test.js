/**
 * **她能看见吗**那一支探针的用例（`scripts/home/probe-vision.js`）。
 *
 * 判据分两层，与探针自己的口径一致：
 * ① **纯函数**（读 PNG 文件头 / 读账本里的 observed 块 / 回执读数 / 字节指纹与扫描 /
 *    主判据 / 四条裁决）—— 直接量，**不碰网络、不起世界**；
 * ② **四条判据的裁决表**——逐条证伪一次（只把一条弄假，结论必须跟着假）。
 *
 * ⚠ 真机那一段（起世界 / 起宿主 / 上传 / 等她回）**不在这里**：它是副作用，跑一次花一次真模型
 * 调用。要跑就 `node scripts/home/probe-vision.js`。
 */
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const probe = require('../probe-vision')

function tempDir(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-vision-${tag}-`))
}

/** 一张"只要文件头是真的"的 PNG：签名 ＋ IHDR 头 16 字节（够 `pngSize` 读）。 */
function pngHeader(width, height) {
  const head = Buffer.alloc(24)
  head.writeUInt32BE(0x89504e47, 0)
  head.writeUInt32BE(0x0d0a1a0a, 4)
  head.writeUInt32BE(13, 8)
  head.write('IHDR', 12, 'latin1')
  head.writeUInt32BE(width, 16)
  head.writeUInt32BE(height, 20)
  return head
}

/** 一行账本：`<JSON 正文>\t<8 位十六进制>`（与 `agent/src/store/format.rs` 同形）。 */
function ledgerLine(value) {
  return `${JSON.stringify(value)}\tdeadbeef`
}

test('读 PNG 的感知体量：从 IHDR 读，**不解码像素**；不是 PNG / 太短 ⇒ null', () => {
  assert.deepEqual(probe.pngSize(pngHeader(720, 200)), { width: 720, height: 200 })
  assert.equal(probe.pngSize(Buffer.from('not a png at all, just text')), null)
  assert.equal(probe.pngSize(Buffer.alloc(8)), null, '太短 ⇒ 读不出来，不猜')
  // 负对照：签名对、chunk 名不对 ⇒ 也不认（半个形状比读不到更坏）
  const broken = pngHeader(10, 10)
  broken.write('XXXX', 12, 'latin1')
  assert.equal(probe.pngSize(broken), null)
})

test('账本里的 observed 块：形状以**权威读法**为准（`{label, text}`），读不出来的行**如实计数**', () => {
  const text = [
    ledgerLine({
      kind: 'observed',
      action: 'im_read_attachment',
      blocks: [{ label: 'im_read_attachment', text: 'pixels = 720x200\nform = media, read = whole, more = none\n' }],
    }),
    ledgerLine({ kind: 'injection', stage: 'run-start', blocks: [{ label: 'status', text: 'run 1' }] }),
    ledgerLine({ kind: 'observed', action: 'im_say', blocks: [{ label: 'im_say', text: '说了' }] }),
    'not json at all',
    '',
  ].join('\n')
  const read = probe.observedBlocks(text)
  assert.equal(read.blocks.length, 2, '两条 observed 各一块；injection 不算')
  assert.equal(read.unreadable, 1, '读不出来的那一行要计数 —— "没有这一块"与"这一行读不出来"是两件事')
  assert.equal(read.lines, 4, '空行不算一行')
  assert.equal(read.kinds.observed, 2)
  assert.equal(read.oddBlocks, 0)

  // 负对照：**认不出的块形状要单独计数**，不许静默当成"没有这一块"。
  // （2026-10-17 实测踩过：按两元组读 ⇒ 15 条 observed 全成 0 块，读数看着像"回执不在场"。）
  const odd = probe.observedBlocks(
    ledgerLine({ kind: 'observed', action: 'x', blocks: [['label', 'text']] }),
  )
  assert.equal(odd.blocks.length, 0)
  assert.equal(odd.oddBlocks, 1, '形状认不出的块要如实报出来')
})

test('媒体回执的读数：`form = media` ＋ `pixels = 宽x高`；`unknown` 也算在场但量不出', () => {
  const media = (body) => probe.mediaReading([{ label: 'im_read_attachment', text: body }])
  assert.deepEqual(
    media('room = r, attachment = a, size = 9\npixels = 720x200\nform = media, read = whole, more = none\n'),
    {
      present: true,
      pixels: '720x200',
      label: 'im_read_attachment',
    },
  )
  assert.equal(media('pixels = unknown\nform = media, read = whole, more = none\n').pixels, 'unknown')
  assert.equal(media('lines = 3, read = 1-3, more = none\n').present, false, '文本附件那条回执不算媒体')
  assert.equal(probe.mediaReading([]).present, false)
})

test('字节指纹与扫描：原样落盘、只落一段、被 base64 —— 三种都要抓住', () => {
  const png = Buffer.concat([pngHeader(720, 200), Buffer.from('x'.repeat(400))])
  const needles = probe.imageFingerprints(png)

  const dirty = tempDir('dirty')
  fs.mkdirSync(path.join(dirty, 'store'), { recursive: true })
  fs.writeFileSync(path.join(dirty, 'store', 'raw.bin'), png)
  const rawHit = probe.scanTree(dirty, needles)
  assert.ok(rawHit.hits.length > 0, '整份原样落盘必须被抓住')
  assert.equal(rawHit.files, 1)

  const middle = png.subarray(Math.floor(png.length / 2), Math.floor(png.length / 2) + 32)
  const encoded = tempDir('encoded')
  fs.writeFileSync(path.join(encoded, 'b64.txt'), middle.toString('base64'), 'utf8')
  assert.ok(probe.scanTree(encoded, needles).hits.length > 0, 'base64 那一形态也要抓住')

  const clean = tempDir('clean')
  fs.writeFileSync(path.join(clean, 'ledger.ndjson'), ledgerLine({ kind: 'observed', blocks: [] }), 'utf8')
  const cleanHit = probe.scanTree(clean, needles)
  assert.deepEqual(cleanHit.hits, [], '干净的家不许误报')
  assert.equal(cleanHit.files, 1, '扫了几个文件要如实报（0 个文件与 0 命中不是一回事）')
  assert.deepEqual(cleanHit.unreadableFiles, [], '读得到的文件不许进"读不出来"那一格')
  assert.equal(cleanHit.emptyFiles, 0)

  // 0 字节的文件被占着读不出来时，它算"搜过了"——里面不可能藏着指纹（实测：`.writer.lock`）。
  assert.equal(probe.classifyUnreadable(0), 'empty')
  assert.equal(probe.classifyUnreadable(7108), 'unreadable')
  assert.equal(probe.classifyUnreadable(null), 'unreadable', '连大小都读不出来 ⇒ 不许说它是空的')

  for (const dir of [dirty, encoded, clean]) fs.rmSync(dir, { recursive: true, force: true })
})

test('读账本：只读**经历**（`trace.ndjson` ＋ `trace.parts/*.ndjson`），不读 `facts.ndjson`', () => {
  const root = tempDir('ledger')
  fs.mkdirSync(path.join(root, 'trace.parts'), { recursive: true })
  fs.writeFileSync(path.join(root, 'trace.ndjson'), 'A', 'utf8')
  fs.writeFileSync(path.join(root, 'trace.parts', '000000000001.ndjson'), 'B', 'utf8')
  fs.writeFileSync(path.join(root, 'facts.ndjson'), 'C', 'utf8')
  fs.writeFileSync(path.join(root, 'facts.parts.idx'), 'D', 'utf8')
  const text = probe.readLedgers(root).text
  assert.ok(text.includes('A') && text.includes('B'), '经历那一份与它的分片都要读到')
  assert.ok(!text.includes('C'), '事实是另一个存储，不许混进来（否则"没有 kind"那一行读不出是哪一种）')
  assert.deepEqual(probe.readLedgers(root).unreadableFiles, [], '读得到的账本不许报"读不出来"')
  fs.rmSync(root, { recursive: true, force: true })
})

test('主判据：整串算报对，**只报那 8 位**也算；报错了不算', () => {
  const agentId = 'a-1'
  const mark = 'RCPT-ABCD2345'
  const hers = (body) => [{ actor: { id: agentId }, body }]
  assert.equal(probe.reportedMark(hers(`图里写的是 ${mark}`), agentId, mark), true)
  assert.equal(probe.reportedMark(hers('是 ABCD2345'), agentId, mark), true, '不可猜的是那 8 位，只报它也算')
  assert.equal(probe.reportedMark(hers('RCPT-ZZZZ9999'), agentId, mark), false)
  assert.equal(probe.reportedMark(hers('我读不到那张图'), agentId, mark), false)
  // 别人的话不算（判据只看身份 —— 与 probe-recall / probe-loop 同一条）。
  assert.equal(
    probe.reportedMark([{ actor: { id: 'human-1' }, body: mark }], agentId, mark),
    false,
    '房里还有别人说的话，不能算她报的',
  )
})

test('四条裁决：全真才 ok；**每条单独证伪一次**，结论都要跟着假', () => {
  const good = {
    replied: true,
    reported: true,
    media: { present: true, pixels: '720x200', label: 'im_read_attachment' },
    expectedPixels: '720x200',
    scan: { hits: [], files: 12, unreadable: 0 },
  }
  assert.equal(probe.visionVerdict(good).ok, true)
  assert.equal(probe.visionVerdict({ ...good, reported: false }).ok, false, '① 没报对码 ⇒ 不成立')
  assert.equal(
    probe.visionVerdict({ ...good, scan: { hits: [{ file: 'x', label: 'PNG 签名' }], files: 12, unreadable: 0 } }).ok,
    false,
    '② 字节落进了她的环境根 ⇒ 不成立',
  )
  assert.equal(
    probe.visionVerdict({ ...good, media: { present: false, pixels: null, label: null } }).ok,
    false,
    '③ 回执不在场 ⇒ 不成立',
  )
  assert.equal(
    probe.visionVerdict({ ...good, media: { present: true, pixels: 'unknown', label: 'x' } }).ok,
    false,
    '④ 量不出感知体量 ⇒ 不成立（它与"量出来了但不一样"分开报）',
  )
  assert.equal(
    probe.visionVerdict({ ...good, media: { present: true, pixels: '640x480', label: 'x' } }).ok,
    false,
    '④ 与文件头对不上 ⇒ 不成立',
  )
  // 没回信那一态：四条里的每一条都不许被当成通过。
  const silent = probe.visionVerdict({ ...good, replied: false, reported: false })
  assert.equal(silent.replied, false)
  assert.equal(silent.ok, false)
})

test('参数：默认值、`--timeout` 要正整数、`--model-source-id` 要非空、不认识的旗标要拒绝', () => {
  assert.deepEqual(probe.parseArguments([]), { timeoutSeconds: 300, keep: false, modelSourceId: null })
  assert.equal(probe.parseArguments(['--timeout', '60']).timeoutSeconds, 60)
  assert.equal(probe.parseArguments(['--model-source-id', 'ms-2']).modelSourceId, 'ms-2')
  assert.equal(probe.parseArguments(['--keep']).keep, true)
  assert.throws(() => probe.parseArguments(['--timeout', '0']), /正整数/)
  assert.throws(() => probe.parseArguments(['--timeout']), /正整数/)
  assert.throws(() => probe.parseArguments(['--model-source-id', '  ']), /id/)
  assert.throws(() => probe.parseArguments(['--nope']), /不认识的参数/)
})

test('给她的那一句：**只有事实与规矩**，且不含任何码（码只在像素里）', () => {
  assert.match(probe.TASK_LINE, /读附件/)
  assert.ok(!/RCPT-[A-Z2-9]{8}/.test(probe.TASK_LINE), '正文里不许出现一个真的码')
  // 画布尺寸是判据 ④ 的期望值来源之一 ⇒ 它得是个能读的宽高。
  assert.ok(probe.CANVAS.width > 0 && probe.CANVAS.height > 0)
})
