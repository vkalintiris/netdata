import { describe, expect, it } from 'vitest'
import { POINTS, snap } from './page-state'

describe('snap', () => {
  it('stretches a fixed window to whole points, keeping its end', () => {
    expect(snap({ after: 1000, before: 2025 })).toEqual({ after: 2025 - 9 * POINTS, before: 2025 })
  })
  it('leaves relative windows alone', () => {
    expect(snap({ after: -900, before: 0 })).toEqual({ after: -900, before: 0 })
  })
})
