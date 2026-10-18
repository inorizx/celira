/**
 * **本机服务**那一格（§8.3 的总开关）。
 *
 * 宪法原话：**面板上给总开关**（`[全部停止]` ＋ `退出时：[保持运行 ▾]`）——"只想留壳子"的用户
 * 一次点击就能做到，**并且知道自己做了什么**；**默认值：不替用户做隐蔽的事**。
 *
 * 三条边界：
 *
 * 1. **只在壳里、只对本机那台出现**：本机服务是"这颗 app 自己起的东西"（§8.1），
 *    浏览器形态没有这一回事，别的机器上的服务也不归这颗 app 停；
 * 2. **读数如实**：世界在不在跑、几支 Agent 在跑，都照壳给的读数说；读不到就说读不到
 *    （**不画成"没在跑"**——缺省不等于零）；
 * 3. **判断不在这里**：三态、可按不可按、那一格怎么写，全在 [`shellLocal`] 的纯函数里
 *    （组件只渲染 —— 与 `agentRowStatus` 同一条先例）。
 *
 * 视觉：沿用运行状态页那套读数行与按钮，不新造展示范式。
 */
import { useCallback, useEffect, useState } from 'react'
import { Loader2, Square } from 'lucide-react'

import { Button } from '@/components/ui/button.tsx'
import {
  canStopLocalServices,
  disableLocalWorld,
  enableLocalWorld,
  localServiceRows,
  localWorldChoiceLabel,
  quitBehaviourLabel,
  readLocalServices,
  readLocalWorld,
  setQuitStopsServices,
  shellInvokeOrNull,
  stopLocalServices,
  type LocalServicesReadout,
  type LocalWorldReadout,
  type ShellInvoke,
} from '../../api/shellLocal.ts'

type Phase = 'loading' | 'ready' | 'absent'

export function LocalServicesPanel() {
  const [invoke, setInvoke] = useState<ShellInvoke | null>(null)
  const [phase, setPhase] = useState<Phase>('loading')
  const [readout, setReadout] = useState<LocalServicesReadout | null>(null)
  /**
   * ★ **「本机世界」那个表态的读数**（2026-10-08）——与上面那份**分开**，因为它们是两份事实：
   * 这一份答"这台机器要不要跑一个世界"（表态），上面那份答"此刻谁在跑"（状态）。
   * 权威各在壳的一处（`local_world_readout` / `local_services_readout`），本件不另算一遍。
   */
  const [world, setWorld] = useState<LocalWorldReadout | null>(null)
  const [busy, setBusy] = useState(false)
  /**
   * **上一步没做成**（`null` = 没这回事）。
   *
   * ⚠ 为什么单开一格：**写失败与读不到是两件事**（前者是"这次没改成"，后者是"根本读不到"）。
   * 从前这里把两者混成一个 `readout === null` ⇒ 一次写失败会把**已经读到的读数也抹掉**，
   * 页面于是从"世界在跑"直接变成"读不到"—— 把一次没做成的动作说成了世界没了。
   */
  const [failure, setFailure] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    void (async () => {
      const seam = await shellInvokeOrNull()
      if (!alive) return
      // ⚠ **`setInvoke(() => seam)`，不是 `setInvoke(seam)`**：壳的接缝**本身是一个函数**，
      // 而 React 见到函数实参就当**更新函数**调用它 ⇒ 存进去的会是"拿旧状态当命令名调一次"的结果
      // （真机验过：面板渲染正常，但三颗按钮一点就报 `invoke is not a function`）。
      setInvoke(() => seam)
      if (!seam) {
        setPhase('absent')
        return
      }
      const first = await readLocalServices(seam)
      if (!alive) return
      setReadout(first)
      // 表态那一份（读不到 ⇒ `null` ⇒ 那一行自己说"读不到"，不冒充"没答过"）。
      const choice = await readLocalWorld(seam)
      if (!alive) return
      setWorld(choice)
      setPhase('ready')
    })()
    return () => {
      alive = false
    }
  }, [])

  /**
   * 一笔写：**读数回来才替换**（失败时保留上一次那份，另说一句"没改成"）。
   *
   * ⚠ 2026-10-08 收成一个泛型：三个开关（全部停止 / 退出时 / 本机世界）走的是**同一条**写路径
   * ——"没应答"那一句、忙态、失败不抹读数，三处各写一遍就是三份会漂的判据。
   */
  const write = useCallback(
    async <T,>(
      action: (seam: ShellInvoke) => Promise<T | null>,
      apply: (value: T) => void,
    ) => {
      if (!invoke) return
      setBusy(true)
      setFailure(null)
      const after = await action(invoke)
      if (after === null) {
        setFailure('这一次没改成 —— 本机服务没应答。读数照旧是上一次那一份。')
      } else {
        apply(after)
      }
      setBusy(false)
    },
    [invoke],
  )

  const onStop = useCallback(
    () => void write((seam) => stopLocalServices(seam), setReadout),
    [write],
  )

  const onQuitBehaviour = useCallback(
    (stops: boolean) => void write((seam) => setQuitStopsServices(seam, stops), setReadout),
    [write],
  )

  /**
   * ★ **改"跑不跑本机世界"这个表态**（2026-10-08）。
   *
   * ⚠ 「不跑」**只改表态** —— 壳那一边的文档写着"只改表态，不停已经在跑的世界"（"停"是
   * 退出语义那一格的事）。所以这里**不**顺手停任何东西，只在下面那句话里指路「全部停止」。
   *
   * ⚠ 「开一个」会**真的把它起起来**（壳的 `local_world_enable` 顺手 `ensure_world` ＋ 带起住户）
   * ⇒ 上面那两行状态读数跟着变，所以顺手重读一次；不重读就会出现"表态说开着、状态说没在跑"。
   */
  const onWorldChoice = useCallback(
    (run: boolean) =>
      void write(
        async (seam) => {
          const after = run ? await enableLocalWorld(seam) : await disableLocalWorld(seam)
          if (after && run) setReadout(await readLocalServices(seam))
          return after
        },
        setWorld,
      ),
    [write],
  )

  // 浏览器形态：这一格**整个不在**（不是"读不到"）—— 本机服务在那儿根本不是一个概念。
  if (phase === 'absent') return null

  const rows = localServiceRows(readout)
  const canStop = canStopLocalServices(readout)

  return (
    <div className="space-y-3 rounded-lg border border-border p-4" data-testid="local-services">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <p className="text-sm font-medium">本机服务</p>
          <p className="text-xs text-muted-foreground">
            这台机器上，Celira 自己起的那些：世界与 Agent 都算。
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!canStop || busy || phase === 'loading'}
          onClick={onStop}
        >
          {busy ? (
            <Loader2 className="size-4 animate-spin" aria-hidden />
          ) : (
            <Square className="size-4" aria-hidden />
          )}
          全部停止
        </Button>
      </div>

      {phase === 'loading' ? (
        <p className="text-xs text-muted-foreground">正在读本机服务…</p>
      ) : (
        <dl className="space-y-1">
          {rows.map((row) => (
            <div key={row.label} className="flex items-baseline gap-2 text-xs">
              <dt className="w-16 shrink-0 text-muted-foreground">{row.label}</dt>
              <dd className={row.tone === 'on' ? 'text-foreground' : 'text-muted-foreground'}>
                {row.value}
              </dd>
            </div>
          ))}
        </dl>
      )}

      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
        <span className="text-xs text-muted-foreground">{quitBehaviourLabel(readout)}</span>
        <div className="flex gap-1">
          <Button
            type="button"
            variant={readout?.quitStopsServices === true ? 'secondary' : 'ghost'}
            size="sm"
            disabled={busy || phase === 'loading'}
            onClick={() => onQuitBehaviour(true)}
          >
            停止
          </Button>
          <Button
            type="button"
            variant={readout?.quitStopsServices === false ? 'secondary' : 'ghost'}
            size="sm"
            disabled={busy || phase === 'loading'}
            onClick={() => onQuitBehaviour(false)}
          >
            保持运行
          </Button>
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        「保持运行」= 关掉这颗 app 之后，世界与 Agent 照旧在后台跑；托盘菜单里那一项也会跟着改名。
      </p>

      {/*
        ★ **「本机世界」那个开关**（2026-10-08，主人当次裁决）。
        从前那个表态**只有第一次那张卡会写**（`App.tsx` 的 `local_world_enable` / `disable`），
        答完之后**改不了** —— 而 `machine_settings` 的文档一直写着"**之后按面板的开关来**"。
        这一格就是把那句话落地：同一个形状（一行读数 ＋ 两颗按钮），与「退出时」并排
        —— 它们本来就是同一类事实（这台机器上的 app 该怎么表现）。
      */}
      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
        <span className="text-xs text-muted-foreground">{localWorldChoiceLabel(world)}</span>
        <div className="flex gap-1">
          <Button
            type="button"
            variant={world?.runLocalWorld === true ? 'secondary' : 'ghost'}
            size="sm"
            disabled={busy || phase === 'loading'}
            onClick={() => onWorldChoice(true)}
          >
            开一个
          </Button>
          <Button
            type="button"
            variant={world?.runLocalWorld === false ? 'secondary' : 'ghost'}
            size="sm"
            disabled={busy || phase === 'loading'}
            onClick={() => onWorldChoice(false)}
          >
            不跑
          </Button>
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        「不跑」只改这个表态 —— 已经在跑的世界<strong>不会被停</strong>（要停，用上面那颗「全部停止」）。
        下次打开 app 时，它会直接带你去连别的世界。
      </p>

      {/* 写失败**另说一句**，不把已经读到的读数抹掉（读不到与没改成是两件事）。 */}
      {failure && (
        <p className="text-xs text-muted-foreground" role="status">
          {failure}
        </p>
      )}
    </div>
  )
}
