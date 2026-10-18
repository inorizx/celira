'use strict'
// **Agent 的后端凭据**按身份分条的本地存储（`credentials.js`）的机械判据。
//
// 这一刀把凭据从"机器上那一个环境变量"搬到「**Agent × 那个世界**」这一层。判据要钉住三件事：
//   1. 目录/文件的形状（按身份分条、一个身份一个文件、身份 id 只允许安全字符）；
//   2. 读的三态（有 / 没有 / 读不出来 —— "没有"与"读不出来"不许混成一件）；
//   3. 取值的**唯一口径** `resolve`（存储 → 环境变量回落），以及"绝不覆盖读不出来的文件"。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const credentials = require('../credentials')

let cases = 0
function credentialCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

/** 一次性凭据目录（用 `CELIRA_CREDENTIALS_DIR` 覆盖，**绝不碰主人真实的那一份**）。 */
function scratch(label) {
  return { CELIRA_CREDENTIALS_DIR: fs.mkdtempSync(path.join(os.tmpdir(), `celira-cred-${label}-`)) }
}

credentialCase('目录：覆盖变量优先；Windows 走 LOCALAPPDATA；别的平台走 XDG/主目录', () => {
  assert.equal(credentials.credentialsDir({ CELIRA_CREDENTIALS_DIR: 'D:/tmp/creds' }), path.resolve('D:/tmp/creds'))
  const win = credentials.credentialsDir({ LOCALAPPDATA: 'C:/Users/x/AppData/Local' })
  if (process.platform === 'win32') {
    assert.equal(win, path.join('C:/Users/x/AppData/Local', 'Celira', 'credentials'))
  }
  const xdg = credentials.credentialsDir({ XDG_CONFIG_HOME: '/home/x/.config' })
  if (process.platform !== 'win32') {
    assert.equal(xdg, path.join('/home/x/.config', 'celira', 'credentials'))
  }
  // `LOCALAPPDATA` 缺席也不许退到当前工作目录（那会把凭据落在随机的项目目录里）。
  const bare = credentials.credentialsDir({})
  assert.ok(path.isAbsolute(bare))
  assert.notEqual(path.dirname(bare), process.cwd())
})

credentialCase('文件名：身份 id 只允许安全字符（它会被拼进文件名）', () => {
  const env = scratch('name')
  assert.ok(credentials.credentialFile('agent-1', env).endsWith(`agent-1${credentials.FILE_SUFFIX}`))
  for (const bad of ['', '   ', '../evil', 'a/b', 'a\\b', 'a b', null, undefined]) {
    assert.equal(credentials.credentialFile(bad, env), null, `${String(bad)} 不该被当文件名`)
  }
})

credentialCase('写读：按身份分条，互不干扰；原文原样回来（trim 掉尾部换行）', () => {
  const env = scratch('rw')
  try {
    const fileA = credentials.write('agent-a', 'token-a', env)
    credentials.write('agent-b', 'token-b', env)
    assert.ok(fileA.includes('agent-a'))
    assert.deepEqual(credentials.read('agent-a', env), { state: 'present', file: fileA, token: 'token-a' })
    assert.equal(credentials.read('agent-b', env).token, 'token-b')
    // 重写 = 换一支（同 key 覆盖），不留临时文件。
    credentials.write('agent-a', 'token-a2', env)
    assert.equal(credentials.read('agent-a', env).token, 'token-a2')
    const leftovers = fs.readdirSync(env.CELIRA_CREDENTIALS_DIR).filter((name) => name.endsWith('.tmp'))
    assert.deepEqual(leftovers, [], '原子写不许留下临时文件')
  } finally {
    fs.rmSync(env.CELIRA_CREDENTIALS_DIR, { recursive: true, force: true })
  }
})

credentialCase('读三态：没有 ⇒ missing；空文件 ⇒ unreadable（**不是**"没有"）', () => {
  const env = scratch('states')
  try {
    assert.equal(credentials.read('nobody', env).state, 'missing')
    const file = credentials.credentialFile('agent-a', env)
    fs.mkdirSync(path.dirname(file), { recursive: true })
    fs.writeFileSync(file, '\n', 'utf8')
    assert.equal(credentials.read('agent-a', env).state, 'unreadable')
  } finally {
    fs.rmSync(env.CELIRA_CREDENTIALS_DIR, { recursive: true, force: true })
  }
})

credentialCase('空凭据不写（空值会让"缺凭据"变成"静默坏掉"）', () => {
  const env = scratch('empty')
  try {
    assert.throws(() => credentials.write('agent-a', '', env), /空的/)
    assert.throws(() => credentials.write('agent-a', '   ', env), /空的/)
    assert.equal(credentials.read('agent-a', env).state, 'missing', '抛了就不许留下文件')
  } finally {
    fs.rmSync(env.CELIRA_CREDENTIALS_DIR, { recursive: true, force: true })
  }
})

credentialCase('读不出来的文件**拒绝覆盖**（那是凭据，不是缓存 —— 不许替主人做决定）', () => {
  const env = scratch('nooverwrite')
  try {
    const file = credentials.credentialFile('agent-a', env)
    fs.mkdirSync(path.dirname(file), { recursive: true })
    fs.writeFileSync(file, '', 'utf8')
    assert.throws(() => credentials.write('agent-a', 'token-new', env), /读不出来/)
    assert.equal(fs.readFileSync(file, 'utf8'), '', '文件一个字节都不许被动')
  } finally {
    fs.rmSync(env.CELIRA_CREDENTIALS_DIR, { recursive: true, force: true })
  }
})

credentialCase('resolve 的唯一口径：存储优先 → 环境变量回落 → 没有', () => {
  const env = scratch('resolve')
  try {
    // 都没有
    assert.deepEqual(credentials.resolve('agent-a', { fallback: null, env }), {
      token: null,
      source: null,
      state: 'missing',
    })
    // 只有环境变量（老路 / 手工路）⇒ 用它，并如实标出来源（调用方据此做迁移）
    const fromEnv = credentials.resolve('agent-a', { fallback: 'token-env', env })
    assert.equal(fromEnv.token, 'token-env')
    assert.equal(fromEnv.source, 'env')
    // 存储里有了 ⇒ 存储赢（一份事实一个权威位置）
    credentials.write('agent-a', 'token-store', env)
    const fromStore = credentials.resolve('agent-a', { fallback: 'token-env', env })
    assert.equal(fromStore.token, 'token-store')
    assert.equal(fromStore.source, 'store')
    // 存储读不出来 ⇒ **不回落**（回落会把"读不出来"伪装成"用环境里那支"，掩盖真问题）
    fs.writeFileSync(credentials.credentialFile('agent-a', env), '', 'utf8')
    const broken = credentials.resolve('agent-a', { fallback: 'token-env', env })
    assert.equal(broken.token, null)
    assert.equal(broken.state, 'unreadable')
  } finally {
    fs.rmSync(env.CELIRA_CREDENTIALS_DIR, { recursive: true, force: true })
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 7, `用例数 ${cases} 少于声明的 7`)
})
