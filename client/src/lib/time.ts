/**
 * 时间显示（**中立层**；TASK_64 从 `api/host.ts` 搬出来）。
 *
 * 为什么单独一个文件：这个函数原先住在带 `host` 名字的模块里，而它与宿主无关
 * （纯日期格式化）。Host 全托管路径退役时它被判定为**中性 helper**——
 * 删模块会把它的消费者一起带走，所以先搬。
 *
 * 纪律：**读不出来就如实说读不出来**。`null` / 空串 / 解析不出来一律给 `—`
 * （长破折号，与页面上其它"没有这个事实"的位置同一符号），绝不
 * `new Date()` 兜底成"现在"——那会把一个缺失的事实说成一个假的事实。
 */

/** 把 ISO 时间串显示给人看；读不出来时给 `—`。 */
export function formatRuntimeTime(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString()
}
