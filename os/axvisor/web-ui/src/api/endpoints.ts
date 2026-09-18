//! Fixed control-plane paths, in one place.
//!
//! `GET /api/manifest` declares capabilities, not links: the control plane is a
//! local process with a stable route table, so a panel asks this module instead
//! of spelling a path out. The QEMU case asserts that the served bundle carries
//! these literals, so a path that drifts away from the backend fails a test
//! instead of turning into a 404 in someone's browser.

import type { VmAction } from './types'

/** Management lane route (`network_console::layout::MANAGEMENT_ROUTE`). */
export const MANAGEMENT_ROUTE = 'axvisor'

export const endpoints = {
  /** Capability manifest of this build. */
  manifest: '/api/manifest',
  /** Guest registry: the authoritative list. */
  vms: '/api/vms',
  /** One guest, including its vCPU states. */
  vm: (id: number) => `/api/vms/${id}`,
  /** Pool directory scan; only an `fs` build answers it. */
  pool: '/api/vms/pool',
  /** Store a pasted config as a pool file (`POST`, `fs` builds only). */
  poolSave: '/api/vms/pool',
  /** Browse one directory of the guest filesystem (`fs` builds only). */
  browse: (path: string) => `/api/vms/browse?path=${encodeURIComponent(path)}`,
  /** Create a guest from a TOML document or from a config path. */
  create: '/api/vms/create',
  /** Lifecycle action on an existing guest. */
  action: (id: number, action: VmAction) => `/api/vms/${id}/${action}`,
  /** Console lanes of this build. */
  consoles: '/api/consoles',
  /** Live registry feed; only a `browser-console` build answers it. */
  events: '/ws/events',
  /** One console lane, or the management shell. */
  terminal: (route: string) => `/ws/${route}`,
  /** The management lane, which the box's serial console shares. */
  managementTerminal: `/ws/${MANAGEMENT_ROUTE}`,
} as const
