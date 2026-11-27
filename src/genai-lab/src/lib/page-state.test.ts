import { describe, expect, it } from 'vitest'
import { ALL_KINDS, defaultKinds, POINTS, snap } from './page-state'

describe('snap', () => {
  it('stretches a fixed window to whole points, keeping its end', () => {
    expect(snap({ after: 1000, before: 2025 })).toEqual({ after: 2025 - 9 * POINTS, before: 2025 })
  })
  it('leaves relative windows alone', () => {
    expect(snap({ after: -900, before: 0 })).toEqual({ after: -900, before: 0 })
  })
})

describe('defaultKinds', () => {
  it('reads an empty Kind filter as model calls on Breakdown and as every kind elsewhere', () => {
    expect(defaultKinds('/breakdown')).toEqual(['model call'])
    expect(defaultKinds('/traces')).toEqual(ALL_KINDS)
  })
})
