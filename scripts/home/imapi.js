'use strict'
/**
 * 对**家里那个 im 服务端**（loopback）说几句话 —— 布置阶段用。
 *
 * 为什么手搓 `http.request` 而不是 `fetch`：
 * 1. **代理**：`fetch` 行为随 Node 版本与 `HTTP_PROXY`/`NODE_USE_ENV_PROXY` 变，
 *    而这里是 loopback 请求——绝不能被代理拦掉（bash 版为此专门写 `--noproxy '*'`）。
 *    `node:http` 不走任何代理，结构性地没这个问题。
 * 2. **不需要** TLS/重定向/压缩那一整套，只要"发一个 JSON、读回一个 JSON"。
 *
 * 纪律：HTTP 层的错误（4xx/5xx）**不是异常**，是读数（调用方按状态码判断、
 * 把响应体原样带出来，方便主人看到服务端到底说了什么）；只有连不上才是异常。
 */

const http = require('node:http')

/**
 * @returns {Promise<{status:number, body:any, error?:string}>} `status=0` 表示连不上。
 */
function imRequest(base, method, pathname, { bearer, key, body, timeoutMs = 20000 } = {}) {
  return new Promise((resolve) => {
    const payload = body === undefined ? null : Buffer.from(JSON.stringify(body), 'utf8')
    const headers = {}
    if (bearer) headers.Authorization = `Bearer ${bearer}`
    if (key) headers['Idempotency-Key'] = key
    if (payload) {
      headers['Content-Type'] = 'application/json'
      headers['Content-Length'] = String(payload.length)
    }

    let settled = false
    const done = (value) => {
      if (!settled) {
        settled = true
        resolve(value)
      }
    }

    let request
    try {
      request = http.request(`${base}${pathname}`, { method, headers, timeout: timeoutMs }, (res) => {
        const chunks = []
        res.on('data', (chunk) => chunks.push(chunk))
        res.on('end', () => {
          const raw = Buffer.concat(chunks).toString('utf8')
          let parsed = null
          try {
            parsed = raw ? JSON.parse(raw) : {}
          } catch {
            parsed = { raw }
          }
          done({ status: res.statusCode ?? 0, body: parsed })
        })
      })
    } catch (error) {
      done({ status: 0, body: null, error: String(error && error.message) })
      return
    }

    request.on('timeout', () => {
      request.destroy()
      done({ status: 0, body: null, error: 'timeout' })
    })
    request.on('error', (error) => done({ status: 0, body: null, error: String(error && error.message) }))
    if (payload) request.write(payload)
    request.end()
  })
}

module.exports = { imRequest }
