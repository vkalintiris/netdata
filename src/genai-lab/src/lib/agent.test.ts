import { describe, expect, it } from 'vitest'
import { clockAligned } from './agent'

describe('clockAligned', () => {
  it('cuts a window that follows now into whole steps on multiples of the step', () => {
    // 900 s in 120 points: 8 s steps, 112 of them, ending on the last multiple of 8 before now.
    expect(clockAligned({ window: { after: -900, before: 0 }, points: 120 }, 1_000_003)).toEqual({
      window: { after: 1_000_000 - 896, before: 1_000_000 },
      points: 112,
    })
    // A few seconds later the earlier points cover the same seconds.
    expect(clockAligned({ window: { after: -900, before: 0 }, points: 120 }, 1_000_007).window.before).toBe(1_000_000)
    // On a boundary the window ends one step earlier: the Agent would shift a window that reaches the current second.
    expect(clockAligned({ window: { after: -900, before: 0 }, points: 120 }, 1_000_000).window.before).toBe(999_992)
    expect(clockAligned({ window: { after: -3600, before: 0 }, points: 120 }, 1_000_019)).toEqual({
      window: { after: 999_990 - 3600, before: 999_990 },
      points: 120,
    })
  })

  it('leaves totals and fixed windows as asked', () => {
    const total = { window: { after: -900, before: 0 }, points: 1 }
    expect(clockAligned(total, 1_000_003)).toEqual(total)
    const fixed = { window: { after: 1_000_000, before: 1_000_960 }, points: 120 }
    expect(clockAligned(fixed, 2_000_000)).toEqual(fixed)
  })
})
