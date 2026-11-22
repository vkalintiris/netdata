import { describe, expect, it } from 'vitest'
import type { Series } from './agent'
import { toolRows } from './tools'

function onePoint(values: Record<string, number>): Series {
  return {
    labels: Object.keys(values),
    times: [100],
    values: Object.values(values).map((v) => [v]),
    units: '',
    pointSeconds: 60,
    after: 40,
    before: 100,
  }
}

describe('toolRows', () => {
  it('joins calls, failures and durations per tool, busiest first', () => {
    const rows = toolRows(
      onePoint({ 'success,search': 3, 'error,search': 1, 'success,check_guess': 10, 'error,check_guess': 0 }),
      onePoint({
        '0.01,search': 0,
        '1.28,search': 2,
        '2.56,search': 2,
        '+Inf,search': 0,
        '0.01,check_guess': 10,
        '1.28,check_guess': 0,
        '2.56,check_guess': 0,
        '+Inf,check_guess': 0,
      }),
      onePoint({ search: 2.2, check_guess: 0.001 }),
    )
    expect(rows).toEqual([
      // Every run is under the first bound: the median is only known to be below it, and below the slowest run.
      { name: 'check_guess', calls: 10, failed: 0, typical: 0.001, typicalIsLimit: true, slowest: 0.001 },
      { name: 'search', calls: 4, failed: 1, typical: 1.28, typicalIsLimit: false, slowest: 2.2 },
    ])
  })
  it('leaves the slowest run open when the Agent has no maximum for the tool', () => {
    const max = onePoint({ search: 0 })
    max.values = [[null]]
    const rows = toolRows(onePoint({ 'success,search': 1 }), onePoint({ '0.01,search': 0, '1.28,search': 1 }), max)
    expect(rows).toEqual([{ name: 'search', calls: 1, failed: 0, typical: 0.645, typicalIsLimit: false, slowest: null }])
  })
  it('keeps a comma in a tool name and drops tools without calls', () => {
    const rows = toolRows(onePoint({ 'success,a,b': 2, 'error,a,b': 0, 'success,idle': 0, 'error,idle': 0 }), undefined, undefined)
    expect(rows).toEqual([{ name: 'a,b', calls: 2, failed: 0, typical: null, typicalIsLimit: false, slowest: null }])
  })
})
