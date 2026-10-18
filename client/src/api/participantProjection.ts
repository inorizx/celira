// 参与者投影（Participant Projection）的**唯一权威**。
//
// 「服务端线协议里的参与者最小视图 → 前端 `Participant` 视图模型」这一个事实，原先有
// 十几份实现：`api/core.ts` 与 `api/store.ts` 各有一份**签名与函数体逐字相同**的
// `toParticipant`，另有若干处把同一个投影**内联重写**在调用点（对象字面量）。同一个
// 事实多份实现迟早打架，本模块是收口后的那一份。
//
// 为什么住在这里（中立面）而不是别处：
//   - 两侧都要用：数据面（`core.ts` 的端点投影、`store.ts` 的状态投影）与呈现面
//     （页面/组件里的「我」与成员投影）共同 import 同一个函数；
//   - **不属于 `fixtures/`**（那是演示数据）：本模块对 `fixtures/` 零依赖，也不含
//     任何演示通道判据 —— `fixtureFallbackAllowed()` 之类的闸门**不许混进真实投影**。
//     演示身份要进世界，只走 `participantDirectory` 的登记 + 闸门那一条路；
//   - 纯函数、无副作用、可 `node --test` 直测（见 `tests/participant-projection.test.ts`）。
//
// ⚠ `signature` / `since` 投影成**空字符串**：线协议这一侧根本没有这两项事实，空串即
// 「未提供」。它与字段缺省在**所有**消费点表现一致（`signature ?? ''`、
// `signature && …`、`since && …`），不要在这里补默认值、也不要去猜内容。

import type { Participant } from './domain'

/**
 * 线协议侧参与者最小视图（`contracts.ts` 的 `ParticipantMinView` / `AgentMinView` /
 * `MeView.participant` 的结构子集）。
 *
 * `kind` 走**窄化判定**（只认 `agent`，其余一律 `human`）：线协议给了词表之外的取值时，
 * 不许把它透传进前端视图模型的联合类型 —— 那等于让服务端的新取值单方面扩大前端类型。
 */
export function toParticipant(v: {
  id: string
  kind: string
  display_name: string
  status?: string
}): Participant {
  return {
    id: v.id,
    kind: v.kind === 'agent' ? 'agent' : 'human',
    displayName: v.display_name,
    initials: v.display_name.slice(0, 1),
    signature: '',
    since: '',
  }
}
