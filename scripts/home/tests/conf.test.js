'use strict'
// `home.conf` 的读与写（CRLF 容错、`mcp_` 前缀剥除、只按第一个等号切）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const { readLines, confGet, confSet, confDelete, mcpDeclarations, mcpNames, portOf, worldBaseOf } = require('../conf')

let cases = 0
function homeCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

function tempFile(text) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-home-conf-'))
  const file = path.join(dir, 'home.conf')
  if (text !== undefined) fs.writeFileSync(file, text, 'utf8')
  return file
}

homeCase('读 LF：取到值；没有的键返回默认值', () => {
  const file = tempFile('listen_addr=127.0.0.1:8080\nclient_port=5173\n')
  assert.equal(confGet(file, 'listen_addr'), '127.0.0.1:8080')
  assert.equal(confGet(file, 'client_port'), '5173')
  assert.equal(confGet(file, 'human_id', '缺省'), '缺省')
  assert.equal(confGet(file, 'human_id'), '')
})

homeCase('读 CRLF：值里**不许**带 CR（记事本写的家也得能读）', () => {
  const file = tempFile('listen_addr=127.0.0.1:8080\r\nclient_port=5173\r\n')
  const listen = confGet(file, 'listen_addr')
  assert.equal(listen, '127.0.0.1:8080')
  assert.equal(listen.length, 14)
  assert.equal(portOf(listen), '8080')
})

homeCase('混合行尾 + 末尾没有换行：照样读得对', () => {
  const file = tempFile('a=1\r\nb=2\nc=3')
  assert.equal(confGet(file, 'a'), '1')
  assert.equal(confGet(file, 'b'), '2')
  assert.equal(confGet(file, 'c'), '3')
})

homeCase('值里含等号：只按**第一个**等号切', () => {
  const file = tempFile('mcp_x=stdio:npx pkg --flag=a=b\n')
  assert.equal(confGet(file, 'mcp_x'), 'stdio:npx pkg --flag=a=b')
})

homeCase('只认行首的键（值里出现的 key= 不算一行）', () => {
  const file = tempFile('note=listen_addr=1.2.3.4\n')
  assert.equal(confGet(file, 'listen_addr', '缺省'), '缺省')
})

homeCase('写：保留其它行（含 mcp_），同键替换，新键追加，写回仍是 LF', () => {
  const file = tempFile('listen_addr=127.0.0.1:8080\r\nmcp_a=stdio:cmd a\r\nroom_id=old\r\n')
  confSet(file, 'room_id', 'new')
  confSet(file, 'agent_id', 'A1')
  const text = fs.readFileSync(file, 'utf8')
  assert.equal(text, 'listen_addr=127.0.0.1:8080\nmcp_a=stdio:cmd a\nroom_id=new\nagent_id=A1\n')
  assert.equal(confGet(file, 'room_id'), 'new')
  assert.equal(confGet(file, 'agent_id'), 'A1')
  assert.deepEqual(mcpDeclarations(file), ['a=stdio:cmd a'])
})

homeCase('写：文件不存在时直接建（写一条键）', () => {
  const file = tempFile()
  fs.rmSync(file, { force: true })
  confSet(file, 'listen_addr', '127.0.0.1:9000')
  assert.equal(confGet(file, 'listen_addr'), '127.0.0.1:9000')
})

homeCase('删：整行删掉、其它行保留；没有这一行 ⇒ 一个字都不动（也不建文件）', () => {
  const file = tempFile('listen_addr=127.0.0.1:8080\r\nagent_id=A1\r\nroom_id=R1\r\n')
  assert.equal(confDelete(file, 'agent_id'), true)
  assert.equal(confGet(file, 'agent_id', '缺省'), '缺省')
  assert.equal(confGet(file, 'room_id'), 'R1', '别的行一个字不动')
  assert.equal(fs.readFileSync(file, 'utf8'), 'listen_addr=127.0.0.1:8080\nroom_id=R1\n', '写回仍是 LF')
  // 负对照：没有这一行 ⇒ 不写盘（mtime 也是读数的一部分），也不凭空建文件。
  const stamp = fs.statSync(file).mtimeMs
  assert.equal(confDelete(file, 'agent_id'), false)
  assert.equal(fs.statSync(file).mtimeMs, stamp)
  const missing = tempFile()
  fs.rmSync(missing, { force: true })
  assert.equal(confDelete(missing, 'agent_id'), false)
  assert.ok(!fs.existsSync(missing), '不存在的文件不该被建出来')
})

homeCase('mcp 声明：剥 `mcp_` 前缀、按文件顺序、同名**不**去重', () => {
  const file = tempFile(
    'mcp_browser=stdio:npx @playwright/mcp --browser chrome\r\n' +
      'mcp_notes=stdio:C:/tools/notes.exe,read-only\r\n' +
      'mcp_other=stdio:cmd,timeout=30\n' +
      'mcp_browser=stdio:again\r\n',
  )
  assert.deepEqual(mcpDeclarations(file), [
    'browser=stdio:npx @playwright/mcp --browser chrome',
    'notes=stdio:C:/tools/notes.exe,read-only',
    'other=stdio:cmd,timeout=30',
    'browser=stdio:again',
  ])
})

homeCase('mcp 声明：`mcp_` 后面什么都没有 ⇒ 走掉；`mcp_=` ⇒ 原样交出 `=`（不校验）', () => {
  // 口径与 bash 版一致：`mcp_` 只是**配置前缀**，等号右边原样交给宿主——
  // 解析只有一处权威（宿主入口）。所以 `mcp_=` 这种畸形行会变成 `--mcp =`，
  // 由宿主在日志里说清；本层**不**替它猜、也不静默吞掉。
  const file = tempFile('mcp_\nmcp_=\nmcp_ok=stdio:x\n')
  assert.deepEqual(mcpDeclarations(file), ['=', 'ok=stdio:x'])
})

homeCase('mcp 名字：只要名字（命令里有空格/逗号都不干扰）', () => {
  const file = tempFile('mcp_browser=stdio:npx a b c,read-only\nmcp_x=stdio:y\n')
  assert.deepEqual(mcpNames(file), ['browser', 'x'])
})

homeCase('readLines：空文件、只有换行、读不到文件都返回空数组', () => {
  assert.deepEqual(readLines(tempFile('')), [])
  assert.deepEqual(readLines(tempFile('\n')), [''])
  assert.deepEqual(readLines(path.join(os.tmpdir(), 'celira-not-here', 'home.conf')), [])
})

homeCase('portOf：IPv4 / 主机名 / 只有端口', () => {
  assert.equal(portOf('127.0.0.1:8080'), '8080')
  assert.equal(portOf('localhost:5173'), '5173')
  assert.equal(portOf('8080'), '8080')
})

// **世界地址由那个家自己说了算**（2026-10-11）：真机门禁从前把 `127.0.0.1:8080` 写死在代码里，
// 于是主人开着自己的家时那条门禁根本跑不了（更糟的形状：临时世界起在 8081，探针却去跟 8080 上的
// **别人的世界**说话）。这条判据把"读不到就拒绝跑、绝不回落默认口"钉死。
homeCase('worldBaseOf：读那个家自己的 listen_addr，拼成世界地址', () => {
  const file = tempFile('listen_addr=127.0.0.1:8123\nclient_port=5173\n')
  assert.equal(worldBaseOf(path.dirname(file)), 'http://127.0.0.1:8123')
})

homeCase('worldBaseOf：远端形态（带协议）原样交回，不重复拼 http://', () => {
  const file = tempFile('listen_addr=https://world.example:8443\n')
  assert.equal(worldBaseOf(path.dirname(file)), 'https://world.example:8443')
})

homeCase('★ worldBaseOf：读不到就是 null —— **绝不回落到默认的 8080**', () => {
  // 三种"读不到"：文件不在、文件在但没有那一键、那一键是空的。
  assert.equal(worldBaseOf(path.join(os.tmpdir(), 'celira-not-here-2')), null)
  assert.equal(worldBaseOf(path.dirname(tempFile('client_port=5173\n'))), null)
  assert.equal(worldBaseOf(path.dirname(tempFile('listen_addr=\n'))), null)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 14, `用例数 ${cases} 少于声明的 14`)
})
