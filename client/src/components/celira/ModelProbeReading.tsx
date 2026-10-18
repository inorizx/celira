import type { HomeConfigProbe } from '@/api/homeConfig'

/**
 * **试连那一步的三件读数**（从 `pages/ModelProvidersPage.tsx` 搬出来的，2026-10-12 第三刀）。
 *
 * 为什么单独一处：那一页 1602 行、主函数 645 行，里面混着**三个主语**（这份清单 / 某一家的形状 /
 * 试连读数）⇒ 按主语拆。这一处只说**试连**：打向哪里、回来的是什么、三档怎么分。
 * ⚠ **纯搬运**：文案与分支逐字照搬，行为零变化。
 */

/** 连接测试读数的人话（**服务方给的机械读数 → 一句人话**，不发明结论）。 */
export function probeText(probe: HomeConfigProbe): string {
  if (probe.status !== null) return `对方回应了 HTTP ${probe.status}`
  return probe.reason === 'timeout' ? '连接超时（对方没有及时回应）' : '连不上（地址不通或没有回应）'
}

/**
 * 「获取模型列表」那一步的读数（`POST /home-config/probe` 回来的那一份）。
 *
 * ⚠ **试连失败是读数不是错误**：连不上 / 对方回 401 都**不影响保存**，所以这里说的是
 * "还没保存、可以接着试"，而不是"失败了"。三档如实分开（与保存后那条通知同款）：
 * 连不上 / 连上了但没给目录 / 连上了且给了目录。
 */
export function ProbeReadingLine({ probe }: { probe: HomeConfigProbe }) {
  if (!probe.ok) {
    return (
      <p className="text-caption text-presence-timeout">
        连接测试：{probeText(probe)}（{probe.url}）—— 还没保存；可以换把钥匙或改地址再试。
      </p>
    )
  }
  if (probe.models.length === 0) {
    return (
      <p className="text-caption text-muted-foreground">
        连上了（{probeText(probe)}），但这家的接口没给模型列表 —— 在下面手动填模型名就行。
      </p>
    )
  }
  return (
    <p className="text-caption text-presence-online">
      连上了（{probeText(probe)}），这家列了 {probe.models.length} 个模型 —— 在下面挑一个。
    </p>
  )
}

/**
 * **「试连会打向这里」的一行小字** —— 让用户**当场看见**自己填的地址会变成什么。
 *
 * 为什么值得占一行：我们让用户填的是**完整线协议地址**（`…/v1/chat/completions`），
 * 而试连实际打的是**模型目录**（`…/models`）。中间有一次「摘掉线协议段再拼」的换算 ——
 * **用户看不见换算，就不知道自己填错的后果**（比如只填到域名 ⇒ 试连会去打 `https://x.com/models`，
 * 那多半不存在）。把它显出来，**填错当场暴露**，不用等按了「获取模型列表」才发现。
 *
 * ⚠ 只在地址**已经像个 http(s) 地址**时才显示：空串或半截地址显出来只会让人困惑。
 */
export function ProbeUrlHint({ url }: { url: string }) {
  if (!url || !/^https?:\/\//i.test(url)) return null
  return (
    <p className="px-1 font-mono text-micro text-muted-foreground">
      试连会打向：<span className="break-all">{url}</span>
    </p>
  )
}
