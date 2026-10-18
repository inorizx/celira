// 进门的两条路（客户端侧纯逻辑 + 匿名兑换）。
//
// 契约：`docs/contracts/core-v1/HOME_ACCESS.md`（Pairing Grant 语义表）。
// **不做匿名自注册**：一个世界的入口只有两条——**建一个世界**（部署），或
// **被请进一个世界**（邀请码）。邀请码是"家钥匙"在人身上的对称物：一次性、
// 有 TTL、可撤销、签发者留痕。
//
// 这个文件里没有 React、没有 store：**纯逻辑可 `node --test` 直测**，
// 只有 `exchangeInvite` 一个副作用函数（而且它在登录之前就要能用，
// 所以显式接收 `baseUrl`，不读会话状态）。

import { fetchJson } from './client.ts'
import type { InviteExchangeResult } from './contracts'

/** 邀请码前缀与总长度（服务端 `domain/grant_token.rs`：`clr_pg_` + 43 字符）。 */
export const INVITE_CODE_PREFIX = 'clr_pg_'
export const INVITE_CODE_LENGTH = 50

/** 邀请码格式检查；不合法返回**给人看的一句话**，合法返回 null。 */
export function inviteCodeError(raw: string): string | null {
  const code = raw.trim()
  if (code === '') return '把邀请码粘进来（它是一串 clr_pg_ 开头的字符）。'
  if (!code.startsWith(INVITE_CODE_PREFIX)) {
    return '邀请码看起来不对：它应该以 clr_pg_ 开头。'
  }
  if (code.length !== INVITE_CODE_LENGTH) {
    return '邀请码长度不对，可能少粘了一段——请整串复制。'
  }
  return null
}

/** 名字检查（服务端同一条：1..=80、禁空、禁 NUL）；不合法返回人话。 */
export function inviteNameError(raw: string): string | null {
  const name = raw.trim()
  if (name === '') return '给自己起个名字——世界里的人会看到它。'
  if (name.includes('\u0000')) return '名字里不能有不可见字符。'
  if ([...name].length > 80) return '名字太长了（最多 80 个字）。'
  return null
}

/** 进门失败时给用户的一句话（按服务端稳定错误码分类，不猜）。 */
export function inviteExchangeErrorText(code: string | undefined): string {
  switch (code) {
    case 'grant_invalid':
      return '这个邀请码用不了了：它可能已经被用过、过期了，或者被邀请人撤销了。请让对方再发一个。'
    case 'invalid_request':
      return '这个邀请码要求你自己填名字——把名字补上再试一次。'
    case 'authentication_required':
      return '这个邀请码不完整，请整串复制后再试。'
    default:
      return '没能用这个邀请码进来：可能是地址填错了，或者那台服务器现在不可达。'
  }
}

/**
 * 用邀请码进门（**匿名**，登录之前就要能用）：成功即拿到一支属于**你**的
 * Human Session token。
 *
 * 调用方负责把它存进本机的连接偏好（与手工粘 token 走同一条路）——这里
 * 不碰存储，也不碰界面。
 */
export async function exchangeInvite(
  baseUrl: string,
  code: string,
  displayName: string,
): Promise<InviteExchangeResult> {
  const trimmed = code.trim()
  return fetchJson<InviteExchangeResult>('/v1/home/pairing-grants/exchange', {
    baseUrl,
    method: 'POST',
    // 匿名端点：Bearer 位置放的是**邀请码**（它本身就是一次性凭据）。
    token: trimmed,
    body: { display_name: displayName.trim() },
  })
}
