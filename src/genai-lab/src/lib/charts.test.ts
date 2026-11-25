import { describe, expect, it } from 'vitest'
import type { Series } from './agent'
import { bucketTotals, latest, quantileLines } from './charts'

function series(labels: string[], times: number[], values: (number | null)[][]): Series {
  return { labels, times, values, units: '', pointSeconds: 10, after: 0, before: 0 }
}

describe('charts', () => {
  it('draws one quantile line per histogram, null where a point has none', () => {
    const request = series(['1', '2', '+Inf'], [10, 20], [[2, 0], [2, 0], [0, 0]])
    const provider = series(['1', '2', '+Inf'], [20], [[4], [0], [0]])
    expect(quantileLines(0.5, [{ label: 'request', series: request }, { label: 'provider', series: provider }])).toEqual(
      { ...request, labels: ['request', 'provider'], values: [[1, null], [null, 0.5]], units: 'seconds' },
    )
  })

  it('adds up a histogram over the window', () => {
    expect(bucketTotals(series(['1', '+Inf'], [10, 20], [[1, 2], [null, 3]]))).toEqual({ '1': 3, '+Inf': 3 })
  })

  it('reads the newest value of a gauge', () => {
    expect(latest(series(['a', 'b'], [10, 20], [[5, null], [null, null]]))).toEqual({ a: 5 })
  })

});
