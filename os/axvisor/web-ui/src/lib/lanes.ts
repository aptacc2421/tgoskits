//! Console-lane naming shared by the panels.
//!
//! `network_console::layout` names a guest lane `vm-<id>` and keeps the
//! management lane at `axvisor`. Both directions live here so the console panel
//! and any future caller agree on how a lane is derived from a VM and how a VM
//! is recovered from a lane.

/** Route of the guest lane that belongs to `vmId`. */
export function guestRoute(vmId: number): string {
  return `vm-${vmId}`
}

/**
 * VM id of a guest lane route, `null` for the management lane.
 *
 * Returns `null` rather than throwing: routes come from the control plane, and
 * an unfamiliar lane should be shown without a claim about which VM it serves.
 */
export function laneVmId(route: string): number | null {
  const match = /^vm-(\d+)$/.exec(route)
  if (match === null) return null
  const id = Number(match[1])
  return Number.isSafeInteger(id) ? id : null
}
