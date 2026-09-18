import { describe, expect, it } from 'vitest'
import { guestRoute, laneVmId } from './lanes'

describe('lane names', () => {
  it('derives a guest lane from a VM id', () => {
    expect(guestRoute(1)).toBe('vm-1')
    expect(guestRoute(42)).toBe('vm-42')
  })

  it('recovers the VM id of a guest lane', () => {
    expect(laneVmId('vm-1')).toBe(1)
    expect(laneVmId('vm-42')).toBe(42)
  })

  it('reports no VM for the management lane or an unknown shape', () => {
    expect(laneVmId('axvisor')).toBeNull()
    expect(laneVmId('vm-')).toBeNull()
    expect(laneVmId('vm-1x')).toBeNull()
    expect(laneVmId('vmcache')).toBeNull()
  })

  it('round-trips every lane it names', () => {
    for (const id of [1, 2, 7, 8]) {
      expect(laneVmId(guestRoute(id))).toBe(id)
    }
  })
})
