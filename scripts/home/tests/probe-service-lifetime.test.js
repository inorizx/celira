'use strict'
/**
 * `probe-service-lifetime.js` 的**判据**用例（真机部分不在这里跑——那要真世界、真宿主、
 * 一次真模型调用，并真的起一个服务）。
 *
 * 为什么这几条值得单独钉住：这条门禁的判据有两句，**缺一不可**——
 * ① 前提：**她真起了它**（A 读数在听）；② 本判据：**家一关它就没了**（B 读数空了）。
 * 少了①，"端口空"是一句**恒真的废话**（她什么都没起，端口当然空）——所以
 * "她没起"必须报**不适用（退出 2）**，而不是通过。这条纪律与 `probe-zero` 那句
 * "配之前她就回了 ⇒ 前提不成立 ⇒ 不许当通过"同形。
 *
 * 另一条：**"读不到"不许说成"没有"**——端口读数解析不出来时报 `known: false`，
 * 由判据把它归到**孤儿**那一档（退出 4），而不是悄悄当成"端口空了"。
 */

const test = require('node:test')
const assert = require('node:assert/strict')

const {
  parseArguments,
  parseServiceLine,
  lifetimeVerdict,
  stopVerdict,
} = require('../probe-service-lifetime')

test('参数：缺省是"等她回 300 秒、口自己挑、清单取默认家"', () => {
  const options = parseArguments([])
  assert.equal(options.timeoutSeconds, 300)
  assert.equal(options.keep, false)
  assert.equal(options.port, null)
  assert.equal(options.listPath, null)
  assert.equal(options.sourceId, null)
})

test('参数：--timeout / --port 只收正整数，不认识的旗标直接拒绝', () => {
  assert.equal(parseArguments(['--timeout', '60']).timeoutSeconds, 60)
  assert.equal(parseArguments(['--port', '8137']).port, 8137)
  assert.equal(parseArguments(['--keep']).keep, true)
  assert.equal(parseArguments(['--model-source-id', 'ms-2']).sourceId, 'ms-2')
  assert.throws(() => parseArguments(['--port', 'abc']), /正整数/)
  assert.throws(() => parseArguments(['--timeout']), /正整数/)
  assert.throws(() => parseArguments(['--nope']), /不认识的参数/)
  assert.throws(() => parseArguments(['--model-sources', '  ']), /非空值/)
})

test('参数：--task 是一张封闭词表（缺省 own；认不出的取值本地拒绝）', () => {
  assert.equal(parseArguments([]).task, 'own', '缺省量的是「她会怎么起」')
  assert.equal(parseArguments(['--task', 'keep']).task, 'keep')
  assert.equal(parseArguments(['--task', 'keep-stop']).task, 'keep-stop')
  assert.throws(() => parseArguments(['--task', 'fly']), /只认 own/)
  assert.throws(() => parseArguments(['--task']), /只认 own/)
})

test('keep-stop 那一张：判据落在她自己的账上，不在端口上', () => {
  // 前提：账上没有 start ⇒ 不适用（她停掉之后端口本来就该空，「空」不能证明「她起过」）。
  assert.equal(
    stopVerdict({ replied: true, started: false, stopped: false, portFreeAfterHerReply: true }).code,
    2,
  )
  // 她要停、账上没有 stop 那一行 ⇒ 4。
  const noStop = stopVerdict({
    replied: true,
    started: true,
    stopped: false,
    portFreeAfterHerReply: true,
  })
  assert.equal(noStop.code, 4)
  assert.equal(noStop.reading, 'not-stopped:no-stop-row-in-her-own-ledger')
  // 账上说停了、端口还在听 ⇒ 4（读数与承诺对不上）。
  const stillUp = stopVerdict({
    replied: true,
    started: true,
    stopped: true,
    portFreeAfterHerReply: false,
  })
  assert.equal(stillUp.code, 4)
  assert.equal(stillUp.reading, 'not-stopped:still-listening-after-she-stopped-it')
  // 起了、停了、端口空了 ⇒ 0。
  const passed = stopVerdict({
    replied: true,
    started: true,
    stopped: true,
    portFreeAfterHerReply: true,
  })
  assert.equal(passed.code, 0)
  assert.equal(passed.reading, 'passed:she-started-it-saw-it-read-it-and-stopped-it')
  // 她没回 ⇒ 3（如实报，不假装）。
  assert.equal(
    stopVerdict({ replied: false, started: false, stopped: false, portFreeAfterHerReply: true }).code,
    3,
  )
})

test('端口读数：在听、没在听、读不到——三者分得开', () => {
  const listening = parseServiceLine(
    'LISTEN pid=28424 name=python.exe parent=(父进程已不在) cmd="D:\\python\\python.exe" ok.py',
  )
  assert.deepEqual(listening, { known: true, listening: true, pid: 28424 })

  assert.deepEqual(parseServiceLine('NOT-LISTENING'), { known: true, listening: false, pid: null })

  // ⚠ 读不到 ⇒ **不许**当成"没有"（那是把"不知道"说成"没有"）。
  const unreadable = parseServiceLine('(读不到)')
  assert.equal(unreadable.known, false)
  assert.equal(unreadable.listening, null)
})

test('判据：她没回 ⇒ 3（如实报，不假装）', () => {
  const verdict = lifetimeVerdict({
    replied: false,
    started: false,
    afterHomeClosed: false,
    portFreeAtEnd: true,
  })
  assert.equal(verdict.code, 3)
  assert.equal(verdict.reading, 'she-did-not-reply')
})

test('判据：她回了但什么都没起 ⇒ 2 不适用（**前提不成立，不许当通过**）', () => {
  const verdict = lifetimeVerdict({
    replied: true,
    started: false,
    afterHomeClosed: false,
    portFreeAtEnd: true,
  })
  assert.equal(verdict.code, 2)
  assert.equal(verdict.reading, 'not-applicable:she-started-nothing')
})

test('判据：她起了、家关了就没了 ⇒ 0 通', () => {
  const verdict = lifetimeVerdict({
    replied: true,
    started: true,
    afterHomeClosed: false,
    portFreeAtEnd: true,
  })
  assert.equal(verdict.code, 0)
  assert.equal(verdict.reading, 'passed:it-died-with-the-home')
})

test('判据：家关了她起的还在听 ⇒ 4 孤儿（本门禁存在的理由）', () => {
  const verdict = lifetimeVerdict({
    replied: true,
    started: true,
    afterHomeClosed: true,
    portFreeAtEnd: true,
  })
  assert.equal(verdict.code, 4)
  assert.equal(verdict.reading, 'orphan:still-listening-after-the-home-closed')
})

test('判据：读数读不到 ⇒ 归到孤儿那一档（不许悄悄算通过）', () => {
  const verdict = lifetimeVerdict({
    replied: true,
    started: true,
    afterHomeClosed: true,
    portFreeAtEnd: false,
  })
  assert.equal(verdict.code, 4)
})

test('判据：门禁自己都没收干净 ⇒ 4（读数不是通过）', () => {
  const verdict = lifetimeVerdict({
    replied: true,
    started: true,
    afterHomeClosed: false,
    portFreeAtEnd: false,
  })
  assert.equal(verdict.code, 4)
  assert.equal(verdict.reading, 'orphan:gate-could-not-clean-up')
})
