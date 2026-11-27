import { describe, expect, it } from 'vitest'
import { breakdownRows, breakdownSeries, noneOf, ranked } from './breakdown'
import type { ExploreAnswer } from './traces'

const window = { after: 100, before: 120, bucket_start_s: 100, bucket_width_s: 10, buckets: 2 }

// Two buckets; teams `a` and `b` named, one more value folded into `other`, some calls without a team.
const all: ExploreAnswer = {
  window,
  histogram: {
    stack: 'resource.attributes.team',
    dimensions: ['a', 'b'],
    buckets: [
      {
        counts: [3, 1],
        other: 1,
        unset: 2,
        sums: [
          { values: [300, 50], other: 10, unset: 20 },
          { values: [30, 5], other: 1, unset: 2 },
          { values: [3e9, 1e9], other: 1e9, unset: 2e9 },
        ],
      },
      {
        counts: [1, 4],
        other: 0,
        unset: 0,
        sums: [
          { values: [100, 400], other: 0, unset: 0 },
          { values: [10, 40], other: 0, unset: 0 },
          { values: [1e9, 8e9], other: 0, unset: 0 },
        ],
      },
    ],
    sums: [
      { key: 'in', totals: [400, 450], other: 10, unset: 20, total: 880, skipped: 0 },
      { key: 'out', totals: [40, 45], other: 1, unset: 2, total: 88, skipped: 0 },
      { key: 'duration', totals: [4e9, 9e9], other: 1e9, unset: 2e9, total: 16e9, skipped: 0 },
    ],
  },
  rows: { matched: 12, items: [] },
}

// The failed calls: one of `b`, one of the value the full answer folds into `other`, one without a team.
const failures: ExploreAnswer = {
  window,
  histogram: {
    stack: 'resource.attributes.team',
    dimensions: ['b', 'c'],
    buckets: [
      { counts: [1, 1], other: 0, unset: 1 },
      { counts: [0, 0], other: 0, unset: 0 },
    ],
  },
  rows: { matched: 3, items: [] },
}

describe('breakdownRows', () => {
  it('joins counts, failures and sums per value, then what was not named', () => {
    expect(ranked(breakdownRows(all, failures), 'tokens')).toEqual([
      { kind: 'value', value: 'b', calls: 5, failed: 1, tokensIn: 450, tokensOut: 45, seconds: 9 },
      { kind: 'value', value: 'a', calls: 4, failed: 0, tokensIn: 400, tokensOut: 40, seconds: 4 },
      // The failed call of `c` belongs to the values the full answer does not name.
      { kind: 'other', value: 'other values', calls: 1, failed: 1, tokensIn: 10, tokensOut: 1, seconds: 1 },
      { kind: 'unset', value: 'not set', calls: 2, failed: 1, tokensIn: 20, tokensOut: 2, seconds: 2 },
    ])
  })
})

describe('breakdownSeries', () => {
  it('draws the shown values and adds every other value up as the rest, without the calls that lack the attribute', () => {
    expect(breakdownSeries(all, failures, 'calls', ['b'], 'rest')).toEqual({
      labels: ['b', 'rest'],
      times: [100, 110],
      values: [
        [1, 4],
        [4, 1],
      ],
      units: '',
      pointSeconds: 10,
      after: 100,
      before: 120,
    })
  })
  it('adds input and output tokens for the tokens measure, and reads seconds for time', () => {
    expect(breakdownSeries(all, failures, 'tokens', ['a', 'b'], 'rest')?.values).toEqual([
      [330, 110],
      [55, 440],
      [11, 0],
    ])
    expect(breakdownSeries(all, failures, 'time', ['a'], 'rest')?.values).toEqual([
      [3, 1],
      [2, 8],
    ])
  })
  it('reads failed calls from the failures answer', () => {
    expect(breakdownSeries(all, failures, 'failed', ['b'], 'rest')?.values).toEqual([
      [1, 0],
      [1, 0],
    ])
  })
})

describe('noneOf', () => {
  it('answers no failed call for any value or time, over the same values and times', () => {
    const none = noneOf(all)
    expect(breakdownRows(all, none).map((r) => r.failed)).toEqual([0, 0, 0, 0])
    expect(breakdownSeries(all, none, 'failed', ['a'], 'rest')?.values).toEqual([
      [0, 0],
      [0, 0],
    ])
  })
})
