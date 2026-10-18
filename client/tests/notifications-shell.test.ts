// 通知的**两种形态**（`src/api/notifications.ts` + 设置页文案）的源码级守卫。
//
// 运行：cd client && npm test
//
// 为什么这条守卫必须有（2026-10-08 刀的真机读数）：壳里 `Notification` 构造器**在**，
// `permission` 是 `default`，而 `requestPermission()` 回 **`denied`** —— 也就是说
// **Web 那一套在壳里是"看起来有、其实永远开不了"**（wry 注册的权限处理器只放行剪贴板读）。
// 所以壳里必须走原生通知；而"有没有走"这件事在浏览器形态里**看不出来**，
// 只能靠源码守卫钉住。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

const HERE = import.meta.dirname
const REPO = path.resolve(HERE, '..')

const read = (relative: string) => fs.readFileSync(path.join(REPO, relative), 'utf8')

let completed = 0
const DECLARED_CASES = 5

function guardCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

guardCase('壳里那条路存在：`isTauriRuntime()` 分支 + 壳的原生通知命令', () => {
  const source = read('src/api/notifications.ts')
  assert.match(source, /isTauriRuntime\(\)/, '判据必须是"有没有壳"，不是构建变量')
  assert.match(source, /invoke\('notify_message'/, '壳里必须真的叫那条壳命令')
  assert.match(source, /import\('@tauri-apps\/api\/core'\)/, '壳命令要走 core 那条动态 import')
})

guardCase('★ 壳里不许再去问 Web 那一套权限（它在壳里恒 denied，问它等于把假话当真话）', () => {
  const source = read('src/api/notifications.ts')
  assert.match(source, /function shellPermission\(\)[\s\S]*?return 'granted'/)
  // ⚠ 注释里**提到**这个包名是允许的（文件头解释了为什么不用它）；不许的是**真的 import 它**。
  assert.ok(
    !/import\s*\(['"]@tauri-apps\/plugin-notification/.test(source) &&
      !/^import .*@tauri-apps\/plugin-notification/m.test(source),
    '那个插件的 JS 在桌面端只是 window.Notification 的薄包装，壳里救不了这一格',
  )
  // 浏览器那条路必须还在（同一份产物，两种形态各自成立）。
  assert.match(source, /new ctor\(/, '浏览器形态那条路不许被删掉')
})

guardCase('两条路的失败都不许吞：各有 `console.error` 与 `return false`', () => {
  const source = read('src/api/notifications.ts')
  assert.match(source, /壳里的通知没弹出去/, '弹不出去要留原文')
  // 壳里那条 catch 必须明确说没成（不许"出错了当成功"）。
  const catchBlock = /catch \(error\) \{[\s\S]*?console\.error\('Celira：壳里的通知没弹出去'[\s\S]*?\n  \}/.exec(source)
  assert.ok(catchBlock, '壳里那条 catch 不见了')
  assert.match(catchBlock[0], /return false/, 'catch 里要明确说没成')
})

guardCase('★ 那句误导文案在壳里不许再出现（设置页必须分形态）', () => {
  const page = read('src/pages/settings/SettingsNotificationsPage.tsx')
  assert.match(page, /isTauriRuntime\(\)/, '设置页要按形态给下一步')
  assert.match(page, /系统的通知设置/, '壳里该说的是系统通知设置')
  assert.match(page, /窗口不在前台时/, '壳里该说的是"窗口"，不是"页面在后台"')
  assert.ok(
    !/permission === 'denied'\s*\?\s*'浏览器已拒绝通知权限/.test(page),
    'denied 那一档不许还是那句写死的"浏览器设置"',
  )
  assert.ok(
    !page.includes('桌面版接上原生通知后这一项才会生效'),
    '那句已经过期了：桌面版今天就接上了',
  )
})

guardCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
