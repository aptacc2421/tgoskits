//! Guest management panel: the live registry, the configuration pool it can be
//! started from, and the lifecycle actions on both.
//!
//! The panel keeps three facts apart, because the control plane does too:
//!
//! - the **registry** (`GET /api/vms`) is what is running or ready right now,
//! - the **pool** (`GET /api/vms/pool`) is a directory of configs that become
//!   VMs on demand, and a pool entry is *not* a VM until it is started,
//! - the **detail** (`GET /api/vms/{id}`) carries the counters that prove an
//!   action took effect, so every mutation is followed by a settle poll instead
//!   of trusting the 200 that only means "request accepted".

import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  ApiError,
  describeError,
  describeStatus,
  type ActionResult,
  type PanelProps,
  type PoolInfo,
  type VmDetail,
  type VmStatus,
  type VmSummary,
} from '@/api/types'
import { describeCpuAffinity } from '@/lib/vcpu'
import { endpoints } from '@/api/endpoints'
import {
  countersOf,
  settleToTerminalState,
  type LifecycleOp,
} from '@/lib/lifecycle'
import { STATUS_TONE } from '@/lib/status'
import { cn } from '@/lib/utils'

/** How long the registry is polled while no event socket is available. */
const REGISTRY_REFRESH_MS = 2000

export default function VmsPanel({ api, resources = [], focusVm = null }: PanelProps) {
  const [registry, setRegistry] = useState<VmSummary[]>(resources)
  const [pool, setPool] = useState<PoolInfo | null>(null)
  const [poolError, setPoolError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [detail, setDetail] = useState<VmDetail | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [createToml, setCreateToml] = useState('')
  const [createError, setCreateError] = useState<string | null>(null)

  const refreshRegistry = useCallback(async () => {
    try {
      const list = await api.get<VmSummary[]>(endpoints.vms)
      setRegistry(list)
      setError(null)
    } catch (e: unknown) {
      setError(describeError(e))
    }
  }, [api])

  const refreshPool = useCallback(async () => {
    try {
      setPool(await api.get<PoolInfo>(endpoints.pool))
      setPoolError(null)
    } catch (e: unknown) {
      // A build without the `fs` feature has no pool route at all; that is a
      // property of this hypervisor, not a failure of the request.
      setPool(null)
      setPoolError(e instanceof ApiError && e.status === 404 ? null : describeError(e))
    }
  }, [api])

  // The registry the shell injects comes from the event socket; polling is only
  // a fallback for builds whose manifest has no feed (the socket is optional).
  useEffect(() => {
    setRegistry(resources)
    if (resources.length === 0) void refreshRegistry()
  }, [resources, refreshRegistry])

  useEffect(() => {
    void refreshRegistry()
    void refreshPool()
    const timer = window.setInterval(() => {
      void refreshRegistry()
      void refreshPool()
    }, REGISTRY_REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [refreshRegistry, refreshPool])

  const showDetail = useCallback(
    async (id: number) => {
      try {
        setDetail(await api.get<VmDetail>(endpoints.vm(id)))
      } catch (e: unknown) {
        setNote(describeError(e))
        setDetail(null)
      }
    },
    [api],
  )

  useEffect(() => {
    if (focusVm !== null) void showDetail(focusVm)
  }, [focusVm, showDetail])

  /**
   * Runs one mutating operation and then proves it landed.
   *
   * The baseline counters are sampled before the request so the settle poll can
   * require a strict increase; without it a status flip alone would pass for a
   * start that never entered the guest.
   */
  const run = useCallback(
    async (key: string, op: LifecycleOp, id: number, action: () => Promise<unknown>, hint: string) => {
      setBusy(key)
      setNote(hint)
      try {
        const before = await api
          .get<VmDetail>(endpoints.vm(id))
          .then(countersOf)
          .catch(() => ({ guest_entry_count: 0, guest_park_count: 0 }))
        await action()
        const result = await settleToTerminalState(op, before, (signal) =>
          api.get<VmDetail>(endpoints.vm(id), signal),
        )
        setNote(result.ok ? `${op} 完成` : result.message)
        if (result.detail) setDetail(result.detail)
        await refreshRegistry()
        await refreshPool()
      } catch (e: unknown) {
        setNote(describeError(e))
      } finally {
        setBusy(null)
      }
    },
    [api, refreshRegistry, refreshPool],
  )

  const start = (id: number) =>
    run(`start-${id}`, 'start', id, () => api.post<ActionResult>(endpoints.action(id, 'start')), `启动 VM[${id}]…`)

  const stop = (id: number) =>
    run(`stop-${id}`, 'stop', id, () => api.post<ActionResult>(endpoints.action(id, 'stop')), `停止 VM[${id}]…`)

  const pause = (id: number) =>
    run(`pause-${id}`, 'pause', id, () => api.post<ActionResult>(endpoints.action(id, 'pause')), `暂停 VM[${id}]…`)

  const resume = (id: number) =>
    run(`resume-${id}`, 'resume', id, () => api.post<ActionResult>(endpoints.action(id, 'resume')), `恢复 VM[${id}]…`)

  const close = (id: number) =>
    run(`close-${id}`, 'delete', id, () => api.del(endpoints.vm(id)), `关闭 VM[${id}]…`)

  const create = async () => {
    setCreateError(null)
    setBusy('create')
    try {
      const created = await api.post<{ id: number }>(endpoints.create, { toml: createToml })
      setCreateOpen(false)
      setCreateToml('')
      setNote(`已创建 VM[${created.id}]，可用「启动」进入 guest`)
      await refreshRegistry()
      await refreshPool()
    } catch (e: unknown) {
      // The failure stays inside the dialog: the input is wrong, not the page.
      setCreateError(describeError(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="flex flex-col gap-4">
      {error && (
        <Banner tone="error">
          读取 VM 列表失败：{error}
          {registry.length > 0 && ' 下表是最后一次成功读取的结果，不代表当前状态。'}
        </Banner>
      )}
      {note && <Banner tone="info">{busy ? `${note}（等待真实状态收敛）` : note}</Banner>}

      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <div>
            <CardTitle>已注册客户机</CardTitle>
            <CardDescription>
              来自 `GET /api/vms`；启动、暂停等动作会在返回后继续轮询，直到计数器或状态证明它真的生效。
            </CardDescription>
          </div>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" onClick={() => void refreshRegistry()}>
              刷新
            </Button>
            <Button size="sm" onClick={() => setCreateOpen(true)}>
              粘贴配置创建
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <table className="w-full text-sm">
            <thead className="text-left text-xs uppercase text-muted-foreground">
              <tr>
                <th className="py-1">ID</th>
                <th className="py-1">名称</th>
                <th className="py-1">状态</th>
                <th className="py-1">vCPU</th>
                <th className="py-1">内存</th>
                <th className="py-1 text-right">操作</th>
              </tr>
            </thead>
            <tbody>
              {registry.map((vm) => (
                <tr key={vm.id} className="border-t">
                  <td className="py-1 font-mono">{vm.id}</td>
                  <td className="py-1">{vm.name}</td>
                  <td className="py-1">
                    <StatusBadge status={vm.status} />
                  </td>
                  <td className="py-1">{vm.cpu_num}</td>
                  <td className="py-1">{vm.memory_mb} MiB</td>
                  <td className="flex flex-wrap justify-end gap-1 py-1">
                    <Button size="sm" variant="outline" onClick={() => void showDetail(vm.id)}>
                      详情
                    </Button>
                    {canStart(vm.status) && (
                      <Button size="sm" disabled={busy !== null} onClick={() => void start(vm.id)}>
                        启动
                      </Button>
                    )}
                    {canStop(vm.status) && (
                      <Button size="sm" variant="outline" disabled={busy !== null} onClick={() => void stop(vm.id)}>
                        停止
                      </Button>
                    )}
                    {vm.status === 'running' && (
                      <Button size="sm" variant="outline" disabled={busy !== null} onClick={() => void pause(vm.id)}>
                        暂停
                      </Button>
                    )}
                    {vm.status === 'paused' && (
                      <Button size="sm" variant="outline" disabled={busy !== null} onClick={() => void resume(vm.id)}>
                        恢复
                      </Button>
                    )}
                    <Button size="sm" variant="destructive" disabled={busy !== null} onClick={() => void close(vm.id)}>
                      关闭
                    </Button>
                  </td>
                </tr>
              ))}
              {registry.length === 0 && (
                <tr>
                  <td colSpan={6} className="py-3 text-center text-muted-foreground">
                    注册表中没有客户机——从下面的配置池启动一台，或粘贴一份配置创建。
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </CardContent>
      </Card>

      {detail && (
        <Card>
          <CardHeader>
            <CardTitle>
              详情 VM[{detail.id}] {detail.name}
            </CardTitle>
            <CardDescription>
              vCPU 状态与进度计数：`guest_entry_count` 只在 vCPU 真的进入 guest 后增长，
              `guest_park_count` 只在 vCPU 真的 park 后增长——它们才是动作生效的证据。
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-sm">
            <p>
              status=<span className="font-mono">{detail.status}</span> · entry=
              <span className="font-mono">{detail.guest_entry_count ?? '-'}</span> · park=
              <span className="font-mono">{detail.guest_park_count ?? '-'}</span>
            </p>
            <ul className="flex flex-wrap gap-2">
              {(detail.vcpu_states ?? []).map((vcpu) => (
                <li key={vcpu.id} className="rounded border px-2 py-1 font-mono text-xs">
                  vCPU {vcpu.id} · {vcpu.state} · 物理 {describeCpuAffinity(vcpu.phys_cpu_set)}
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle>客户机配置池</CardTitle>
          <CardDescription>
            `GET /api/vms/pool` 列出目录里所有能变成客户机的配置，以及不能变成客户机的原因；
            条目只是候选，`start` 时才会真正创建。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm">
          {poolError && <Banner tone="error">读取配置池失败：{poolError}</Banner>}
          {pool === null && !poolError && (
            <p className="text-muted-foreground">
              这个构建没有客户机配置池（未启用文件系统能力），可以直接粘贴配置创建。
            </p>
          )}
          {pool && (
            <>
              <p className="text-muted-foreground">
                目录 <span className="font-mono">{pool.directory}</span>
              </p>
              <ul className="flex flex-col gap-2">
                {pool.entries.map((entry) => (
                  <li key={`${entry.id}-${entry.path}`} className="rounded border p-2">
                    <div className="flex items-center justify-between gap-2">
                      <span>
                        VM[{entry.id}] <span className="font-mono text-xs">{entry.name}</span>
                      </span>
                      <span className="flex items-center gap-2">
                        <span className="font-mono text-xs text-muted-foreground">{entry.path}</span>
                        <Button
                          size="sm"
                          disabled={busy !== null}
                          onClick={() => void start(entry.id)}
                        >
                          启动
                        </Button>
                      </span>
                    </div>
                  </li>
                ))}
                {pool.entries.length === 0 && (
                  <li className="text-muted-foreground">目录里还没有可启动的配置。</li>
                )}
              </ul>
              <ul className="flex flex-col gap-1">
                {pool.issues.map((issue) => (
                  <li key={`${issue.kind}-${issue.path}`} className="text-xs text-amber-600">
                    无法使用 <span className="font-mono">{issue.path}</span>（{issue.kind}）：{issue.detail}
                  </li>
                ))}
              </ul>
            </>
          )}
        </CardContent>
      </Card>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>粘贴客户机配置</DialogTitle>
            <DialogDescription>
              内容是 TOML 文本，交给 `POST /api/vms/create`，与配置池里的文件走同一条创建路径。
            </DialogDescription>
          </DialogHeader>
          <textarea
            className="h-56 w-full rounded-md border bg-transparent p-2 font-mono text-xs"
            value={createToml}
            spellCheck={false}
            placeholder={'[base]\nid = 9\nname = "guest"\n\n[kernel]\nimage_location = "memory"\n...'}
            onChange={(event) => setCreateToml(event.target.value)}
          />
          {createError && <Banner tone="error">{createError}</Banner>}
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>
              取消
            </Button>
            <Button disabled={busy !== null || createToml.trim().length === 0} onClick={() => void create()}>
              创建
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

/**
 * Which actions the current status allows.
 *
 * The control plane is the authority — it answers 409 for a transition the state
 * machine rejects — but the buttons should not offer a start that is known to be
 * refused: restart-after-stop is explicitly rejected (a fresh vCPU task on an
 * idled pinned CPU never gets scheduled), so `stopped` has no start button.
 */
export function canStart(status: VmStatus): boolean {
  return status === 'ready'
}

export function canStop(status: VmStatus): boolean {
  return status === 'running' || status === 'paused'
}

function StatusBadge({ status }: { status: VmStatus }) {
  return (
    <span className={cn('rounded-full border px-2 py-0.5 text-xs', STATUS_TONE[status] ?? '')}>
      {describeStatus(status)}
    </span>
  )
}

function Banner({ tone, children }: { tone: 'info' | 'error'; children: ReactNode }) {
  return (
    <p
      className={cn(
        'rounded-md border px-3 py-2 text-sm',
        tone === 'error' ? 'border-destructive/40 bg-destructive/10' : 'bg-muted',
      )}
    >
      {children}
    </p>
  )
}
