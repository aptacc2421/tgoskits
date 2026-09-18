//! Terminal view: one console lane on a WebSocket, rendered with xterm.
//!
//! The component is deliberately stateless about *which* lane it shows: the
//! caller passes the route (`/ws/vm-2` for a guest, `/ws/axvisor` for the
//! hypervisor shell), and the panel decides when to mount it. Everything the
//! host console page used to do lives here — chunked binary writes, streaming
//! UTF-8 decoding, fit-on-resize, and a manual reconnect, because a browser
//! WebSocket reports every failed handshake as an anonymous 1006.

import { useCallback, useEffect, useRef, useState } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { ConsoleSocket, type SocketStatus } from '@/api/ws'
import { cn } from '@/lib/utils'

const STATUS_TEXT: Record<SocketStatus, string> = {
  connecting: '连接中…',
  open: '已连接',
  closed: '已断开',
}

export interface TerminalViewProps {
  /** WebSocket route of the lane, e.g. `/ws/vm-1` or `/ws/axvisor`. */
  path: string
  /** Shown in the title bar. */
  title: string
  /** Extra description next to the title (the console's display name). */
  subtitle?: string
  /**
   * `GET /api/consoles` says another browser holds this lane. The lanes are
   * exclusive, so the socket below will simply fail; saying why up front is the
   * difference between "input does nothing" and "close the other page".
   */
  occupied?: boolean
  className?: string
}

export function TerminalView({ path, title, subtitle, occupied, className }: TerminalViewProps) {
  const hostRef = useRef<HTMLDivElement>(null)
  const [status, setStatus] = useState<SocketStatus>('connecting')
  const [detail, setDetail] = useState<string | null>(null)
  const [generation, setGeneration] = useState(0)
  const [counters, setCounters] = useState({ up: 0, down: 0 })

  const reconnect = useCallback(() => setGeneration((value) => value + 1), [])

  useEffect(() => {
    const host = hostRef.current
    if (!host) return

    const terminal = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      lineHeight: 1.1,
      scrollback: 4000,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      theme: { background: '#0b1021', foreground: '#c8d3f5', cursor: '#c8d3f5' },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(host)
    fitSafely(fit)

    let up = 0
    let down = 0
    const socket = new ConsoleSocket(path, {
      onData: (text) => {
        down += text.length
        setCounters({ up, down })
        terminal.write(text)
      },
      onStatus: (next, reason) => {
        setStatus(next)
        setDetail(reason ?? null)
        if (next === 'open') terminal.focus()
      },
    })

    // Input is forwarded verbatim: the shell on the other side echoes and edits
    // its own line buffer, so a local line editor here would double the echo.
    const input = terminal.onData((data) => {
      up += data.length
      setCounters({ up, down })
      socket.send(data)
    })

    // Ctrl+C copies a selection instead of sending the interrupt, matching the
    // host page: the guest/shell keeps its own SIGINT semantics otherwise.
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown' || !event.ctrlKey || event.key.toLowerCase() !== 'c') {
        return true
      }
      if (!terminal.hasSelection()) return true
      event.preventDefault()
      void navigator.clipboard?.writeText(terminal.getSelection()).catch(() => undefined)
      return false
    })

    const observer = new ResizeObserver(() => fitSafely(fit))
    observer.observe(host)

    return () => {
      observer.disconnect()
      input.dispose()
      socket.close()
      terminal.dispose()
    }
  }, [path, generation])

  return (
    <div className={cn('flex h-full min-w-0 flex-col overflow-hidden bg-[#0b1021]', className)}>
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-zinc-800 px-2 py-1 text-xs text-zinc-400">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-mono text-zinc-200">{title}</span>
          {subtitle && <span className="truncate text-zinc-500">{subtitle}</span>}
          <span className={status === 'open' ? 'text-emerald-400' : 'text-zinc-500'}>
            {STATUS_TEXT[status]}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <span
            className="font-mono text-[10px] tabular-nums"
            title="↑ 已上行（输入），↓ 已下行（输出）"
          >
            ↑{counters.up} ↓{counters.down}
          </span>
          <button
            type="button"
            className="rounded px-1.5 py-0.5 text-[10px] text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
            onClick={reconnect}
          >
            重连
          </button>
        </div>
      </div>

      {occupied && status !== 'open' && (
        <p className="bg-amber-950/60 px-2 py-1 text-xs text-amber-300">
          该终端通道已被另一个浏览器页面占用（通道为独占订阅）。关闭占用它的页面后点「重连」。
        </p>
      )}
      {status === 'closed' && detail && (
        <p className="bg-zinc-900 px-2 py-1 text-xs text-zinc-400">{detail}</p>
      )}

      <div ref={hostRef} className="min-h-0 flex-1 p-1" />
    </div>
  )
}

/** `fit()` throws while the container has no size (a hidden tab); that is not an error. */
function fitSafely(fit: FitAddon): void {
  try {
    fit.fit()
  } catch {
    // The ResizeObserver calls this again once the container is visible.
  }
}
