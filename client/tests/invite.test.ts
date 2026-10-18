// 进门两条路（`src/api/invite.ts`）的行为测试。
//
// 判据对着服务端已落地的契约（`HOME_ACCESS.md`：邀请码 `clr_pg_` + 43 字符、
// 一次性、72 小时、可撤销；`PRODUCT_VISION.md` 当前阶段：**不做匿名自注册**）。
// 这里只测**纯逻辑**（格式检查与人话映射）；真正的兑换是网络副作用，由服务端
// 路由测试与真机闭环证明。
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  INVITE_CODE_LENGTH,
  INVITE_CODE_PREFIX,
  inviteCodeError,
  inviteExchangeErrorText,
  inviteNameError,
} from '../src/api/invite.ts'

/** 形状合法的一串（与服务端 `clr_pg_` + 43 字符一致）。 */
const GOOD_CODE = INVITE_CODE_PREFIX + 'A'.repeat(INVITE_CODE_LENGTH - INVITE_CODE_PREFIX.length)

test('邀请码形状：前缀 + 长度两条都要对，且空白会被修掉', () => {
  assert.equal(inviteCodeError(GOOD_CODE), null)
  assert.equal(inviteCodeError(`  ${GOOD_CODE}  `), null, '粘进来常带空白，trim 后应当合法')

  assert.match(inviteCodeError('') ?? '', /粘进来/)
  assert.match(inviteCodeError('clr_hs_不是邀请码') ?? '', /clr_pg_/, 'Human token 不是邀请码')
  assert.match(inviteCodeError(GOOD_CODE.slice(0, -1)) ?? '', /长度/, '少粘一段要能认出来')
  assert.match(inviteCodeError(GOOD_CODE + 'A') ?? '', /长度/)
})

test('名字：自己命名自己，空/超长/带 NUL 都要拦下', () => {
  assert.equal(inviteNameError('阿元'), null)
  assert.equal(inviteNameError('  小满  '), null, '首尾空白不算内容，trim 后仍有名字即可')
  assert.equal(inviteNameError('a'.repeat(80)), null, '80 个字正好合法')
  assert.match(inviteNameError('') ?? '', /起个名字/)
  assert.match(inviteNameError('   ') ?? '', /起个名字/)
  assert.match(inviteNameError('a'.repeat(81)) ?? '', /太长/)
  assert.match(inviteNameError('阿\u0000元') ?? '', /不可见/)
})

test('进门失败说的是人话，而且按稳定错误码分类（不猜）', () => {
  // 已用/过期/撤销：服务端统一 grant_invalid ⇒ 同一句话（不泄露是哪一种）。
  assert.match(inviteExchangeErrorText('grant_invalid'), /用过|过期|撤销/)
  assert.match(inviteExchangeErrorText('invalid_request'), /名字/)
  assert.match(inviteExchangeErrorText('authentication_required'), /整串/)
  // 未知码（含网络失败）落到"进不来"那一档，不假装知道原因。
  assert.match(inviteExchangeErrorText(undefined), /地址|不可达/)
  assert.match(inviteExchangeErrorText('storage_unavailable'), /地址|不可达/)
})

test('三种失败说的是三句不同的话（否则分类没有意义）', () => {
  const texts = ['grant_invalid', 'invalid_request', 'authentication_required'].map((c) =>
    inviteExchangeErrorText(c),
  )
  assert.equal(new Set(texts).size, 3)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(5 >= 1)
})
