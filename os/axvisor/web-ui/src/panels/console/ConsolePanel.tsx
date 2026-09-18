//! Guest terminal panel: one terminal per browser console lane.
//!
//! The lane table is *runtime state*: `GET /api/consoles` is derived from the VM
//! registry, so a lane appears when a VM is created and disappears when it is
//! closed. The panel therefore re-reads the table whenever the registry feed
//! reports a change — that is the whole reason the shell injects the feed here —
//! and drops the terminals whose lane is gone instead of retrying them forever.

import { useEffect, useMemo, useState } from 'react'
import { TerminalView } from '@/components/Terminal'
import { describeError, type ConsoleInfo, type PanelProps } from '@/api/types'
import { endpoints } from '@/api/endpoints'
import { cn } from '@/lib/utils'

/** Route of the guest lane that belongs to `vmId` (`network_console::layout`). */
export function guestRoute(vmId: number): string {
  return `vm-${vmId}`
}

export default function ConsolePanel({ api, resources = [], focusVm = null }: PanelProps) {
  const [consoles, setConsoles] = useState<ConsoleInfo[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [activeRoute, setActiveRoute] = useState<string | null>(null)

  // `resources` changes identity on every registry event, so this is a fetch per
  // change and nothing more: no timer, no polling.
  useEffect(() => {
    let cancelled = false
    api
      .get<ConsoleInfo[]>(endpoints.consoles)
      .then((list) => {
        if (cancelled) return
        setConsoles(list)
        setError(null)
      })
      .catch((e: unknown) => {
        if (cancelled) return
        setConsoles([])
        setError(describeError(e))
      })
    return () => {
      cancelled = true
    }
  }, [api, resources])

  const routes = useMemo(() => (consoles ?? []).map((c) => c.route), [consoles])

  // Guest lanes only: the management shell has its own panel, so showing it here
  // as well would put the same exclusive lane in two places.
  const guestLanes = useMemo(
    () => (consoles ?? []).filter((console) => console.route !== 'axvisor'),
    [consoles],
  )

  useEffect(() => {
    if (activeRoute !== null && routes.includes(activeRoute)) return
    setActiveRoute(guestLanes.length > 0 ? guestLanes[0].route : null)
  }, [routes, guestLanes, activeRoute])

  useEffect(() => {
    if (focusVm === null) return
    const route = guestRoute(focusVm)
    if (routes.includes(route)) setActiveRoute(route)
  }, [focusVm, routes])

  return (
    <div className="flex h-[70vh] min-h-0 flex-col gap-2">
      {error && (
        <p className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
          读取终端清单失败：{error}
        </p>
      )}
      {guestLanes.length === 0 ? (
        <p className="rounded-md border px-3 py-6 text-center text-sm text-muted-foreground">
          当前没有客户机终端通道。浏览器的终端通道随客户机创建而出现、随关闭而释放，
          先到「虚拟机」面板启动一台。
        </p>
      ) : (
        <>
          <div className="flex shrink-0 flex-wrap items-center gap-1">
            {guestLanes.map((console) => (
              <button
                key={console.route}
                type="button"
                onClick={() => setActiveRoute(console.route)}
                className={cn(
                  'rounded-md px-2.5 py-1 font-mono text-xs',
                  console.route === activeRoute
                    ? 'bg-secondary text-secondary-foreground'
                    : 'text-muted-foreground hover:bg-accent',
                )}
              >
                {console.name}
                {console.attached && (
                  <span className="ml-1 text-amber-400" title="已被另一个浏览器页面占用">
                    ●
                  </span>
                )}
              </button>
            ))}
          </div>
          {/* Every lane stays mounted: the lanes are exclusive, so unmounting the
              inactive ones would tear down the session on each tab switch. */}
          {guestLanes.map((console) => (
            <div
              key={console.route}
              className={cn(
                'min-h-0 flex-1',
                console.route === activeRoute ? 'block' : 'hidden',
              )}
            >
              <TerminalView
                path={endpoints.terminal(console.route)}
                title={console.name}
                subtitle={endpoints.terminal(console.route)}
                occupied={console.attached}
              />
            </div>
          ))}
        </>
      )}
    </div>
  )
}
