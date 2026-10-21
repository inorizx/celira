'use strict'
/**
 * 建家那一步必须把**空的技能区**也建出来（2026-10-20）。
 *
 * 为什么单独钉住：Agent 侧（`agent/src/skills_desktop.rs`）把两件事分得很开——
 * `skills/` **不存在** ⇒ 整块不出场（`None`）；存在但一个一级条目都没有 ⇒ 如实说
 * `skills: 0 项（技能区为空）`（`Some(0)`）。⇒ 建家若只建 `materials/` 不建 `skills/`，
 * 主体看到的是"没有那一行"，于是"这具身体没有技能区"与"技能区空着"在主体眼里是同一件事。
 * 真机实测：她据此答成"没有可安装的技能包"（对话记录见 2026-10-20）。
 *
 * 运行： node --test "scripts/home/tests/*.test.js"
 */

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const { ensureHomeDirs } = require('../up')

let cases = 0

test('建家：空技能区也建出来，且重复跑不报错、不写任何东西', () => {
  cases += 1
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-up-dirs-'))
  ensureHomeDirs(home)
  const skills = path.join(home, 'materials', 'skills')
  assert.ok(fs.statSync(skills).isDirectory(), '技能区必须建出来（不是等有人放第一个技能才建）')
  assert.deepEqual(fs.readdirSync(skills), [], '刚建出来是空的——那正是「技能区为空」，不是「没有技能区」')
  // 幂等：第二次跑不能因为目录已存在而抛，也不能往里塞任何东西。
  ensureHomeDirs(home)
  assert.deepEqual(fs.readdirSync(skills), [], '重复建家不许往里写东西')
  fs.rmSync(home, { recursive: true, force: true })
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
})
