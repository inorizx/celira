// 「服务器」页地址那一格的纯逻辑测试（前端填肉刀，2026-09-23）。
//
// 判据只有一条要紧的：**不知道就不标「本机」**。「本机」是一个事实（地址落在 loopback
// 上），地址读不懂、或页面自己那个主机名解析不出来时，只能说"不知道" —— 不许把"不知道"
// 画成"本机"，也不许画成"远程"。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { isLoopbackHost, serverAddressReading } from '../src/lib/server-address.ts'

test('空白地址 = 同源：地址那一格说的是"本页面所在的地址"（并带上它）', () => {
  const reading = serverAddressReading('', 'http://localhost:5173')
  assert.equal(reading.address, '本页面所在的地址（http://localhost:5173）')
  // 同源、而页面自己就在本机上 ⇒ 这一格确实是本机（这是读数，不是猜）。
  assert.equal(reading.local, true)
})

test('空白地址 + 页面在远端 ⇒ **不标**「本机」', () => {
  const reading = serverAddressReading('', 'https://celira.example.com')
  assert.equal(reading.address, '本页面所在的地址（https://celira.example.com）')
  assert.equal(reading.local, false)
})

test('用户填的地址原样上屏，并按主机名判「本机」', () => {
  assert.deepEqual(serverAddressReading('http://127.0.0.1:8080', 'http://localhost:5173'), {
    address: 'http://127.0.0.1:8080',
    local: true,
  })
  // 环回还有 IPv6 这一种（`URL.hostname` 对 IPv6 给带方括号的形状）。
  assert.equal(serverAddressReading('http://[::1]:8080', '').local, true)
  assert.deepEqual(serverAddressReading('https://api.example.com', ''), {
    address: 'https://api.example.com',
    local: false,
  })
})

test('★ 地址读不懂 ⇒ 原样显示、**不标任何东西**（既不说本机也不说远程）', () => {
  const reading = serverAddressReading('这不是一个地址', '')
  assert.equal(reading.address, '这不是一个地址')
  assert.equal(reading.local, false)
  // 只去掉首尾空白（免得那一格出现看不见的空格），其余一个字符不改。
  assert.equal(serverAddressReading('  https://api.example.com  ', '').address, 'https://api.example.com')
})

test('isLoopbackHost：只认字面的三种环回名（大小写不敏感）', () => {
  for (const host of ['localhost', 'LOCALHOST', '127.0.0.1', '::1', '[::1]', ' localhost ']) {
    assert.equal(isLoopbackHost(host), true, host)
  }
  for (const host of ['', '169.254.1.1', 'example.com', 'localhost.example.com', '10.0.0.1']) {
    assert.equal(isLoopbackHost(host), false, host)
  }
})
