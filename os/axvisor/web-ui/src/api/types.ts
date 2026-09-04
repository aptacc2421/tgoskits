//! Contract types shared by the axvisor control plane and this frontend.
//!
//! `shell/` depends only on this module, never on `panels/`: a panel reaches the
//! shell through the `PanelRegistry` contract, so adding a panel never touches
//! the shell (see `panels/registry.ts`).

import type { ComponentType } from 'react'
import type { ApiClient } from './client'

/** One panel node of `GET /api/manifest`. */
export interface PanelMeta {
  kind: string
  title: string
  /** Operations the panel may use: `read`, `write`, `stream`. */
  verbs: string[]
}

/**
 * The capability manifest (`GET /api/manifest`).
 *
 * The backend derives it from the features it was built with, so the navigation
 * follows the build: a panel that is not declared has no backing routes in this
 * binary. `proto` is additive — an older frontend degrades on unknown panels
 * rather than failing, which is what the fallback renderer is for.
 */
export interface Manifest {
  proto: number
  panels: PanelMeta[]
}

/** Context handed to every panel by the shell. */
export interface PanelProps {
  meta: PanelMeta
  api: ApiClient
  /** Live VM registry snapshot (see `api/events.ts`); panels that do not care ignore it. */
  resources?: VmSummary[]
  /** VM the navigation asked the panel to focus, set by clicking a resource entry. */
  focusVm?: number | null
}

export type PanelComponent = ComponentType<PanelProps>

/**
 * Renderer registry contract. The implementation lives in `panels/registry.ts`.
 *
 * `terminalKind` is the kind the shell opens when an operator clicks a resource:
 * it is a registry fact, not a shell fact, because the shell must not know any
 * panel kind. That is what keeps adding a panel out of `shell/`.
 */
export interface PanelRegistry {
  resolve(kind: string): PanelComponent
  terminalKind?: string
}

/**
 * An HTTP failure that carries both dimensions a caller needs: the status code
 * and the backend's own `error` field, read from the body exactly once here.
 */
export class ApiError extends Error {
  readonly status: number
  readonly detail: string

  constructor(status: number, detail: string) {
    super(`HTTP ${status} · ${detail}`)
    this.name = 'ApiError'
    this.status = status
    this.detail = detail
  }
}

export function describeError(e: unknown): string {
  if (e instanceof ApiError) {
    // A body the backend did not fill in still has to read as a sentence: the
    // status code alone already identifies the failure class on this API.
    const detail = e.detail.length > 0 ? e.detail : STATUS_HINTS[e.status] ?? ''
    return detail.length > 0 ? `HTTP ${e.status} · ${detail}` : `HTTP ${e.status}`
  }
  return e instanceof Error ? e.message : String(e)
}

/** What each rejection means on this API, used when the body carries no detail. */
const STATUS_HINTS: Record<number, string> = {
  400: '请求内容无法解析为一份客户机配置',
  404: '目标不存在（未注册，也不在客户机配置池里）',
  409: '当前状态不允许该操作',
  500: '宿主机侧故障，详见串口日志',
  503: '宿主资源不足（内存或浏览器终端通道已用尽）',
}

/** Human-readable VM status; an unknown future state is shown verbatim. */
export function describeStatus(status: VmStatus | string): string {
  return STATUS_TEXT[status as VmStatus] ?? String(status)
}

const STATUS_TEXT: Record<VmStatus, string> = {
  ready: '就绪',
  running: '运行中',
  pausing: '暂停中',
  paused: '已暂停',
  stopping: '停止中',
  stopped: '已停止',
  destroying: '销毁中',
  destroyed: '已销毁',
  failed: '失败',
  unknown: '未知',
}

/** VM lifecycle status strings reported by the control plane (`VmStatus::as_str`). */
export type VmStatus =
  | 'ready'
  | 'running'
  | 'pausing'
  | 'paused'
  | 'stopping'
  | 'stopped'
  | 'destroying'
  | 'destroyed'
  | 'failed'
  | 'unknown'

/** One element of `GET /api/vms`. */
export interface VmSummary {
  id: number
  name: string
  status: VmStatus
  cpu_num: number
  memory_mb: number
}

/** One element of `GET /api/vms/{id}`'s `vcpu_states`. */
export interface VcpuState {
  id: number
  state: string
  phys_cpu_set: number[]
}

/** `GET /api/vms/{id}`: the summary plus per-vCPU state and the progress counters. */
export interface VmDetail extends VmSummary {
  vcpu_states?: VcpuState[]
  /** VM-level aggregate: advances only after a vCPU actually re-entered the guest. */
  guest_entry_count?: number
  /** VM-level aggregate: advances only when a vCPU genuinely parked in the suspend wait. */
  guest_park_count?: number
}

/** Body of a lifecycle action (`start`/`stop`/`pause`/`resume`). */
export interface ActionResult {
  ok: boolean
  status: VmStatus
  /**
   * `stop` and `pause` have request semantics: the response reports the status
   * right after the request was accepted, so the transition may still be in
   * flight. The UI must not treat `ok` as "converged".
   */
  async: boolean
}

export type VmAction = 'start' | 'stop' | 'pause' | 'resume'

/** One element of `GET /api/vms/pool`'s `entries`. */
export interface PoolEntry {
  id: number
  name: string
  path: string
  /** The raw TOML of the entry, so a client can show or prefill it. */
  toml: string
}

/**
 * One file in the pool directory that cannot become a VM, with the reason the
 * scanner rejected it (`empty`, `invalid-toml`, `unreadable`, `duplicate-id`,
 * `missing-image`, `not-a-guest-config`, `directory-unavailable`).
 */
export interface PoolIssue {
  kind: string
  path: string
  detail: string
}

/** `GET /api/vms/pool` (`fs` builds only). */
export interface PoolInfo {
  directory: string
  entries: PoolEntry[]
  issues: PoolIssue[]
}

/** One element of `GET /api/consoles`: a WebSocket route and what it belongs to. */
export interface ConsoleInfo {
  route: string
  name: string
}
