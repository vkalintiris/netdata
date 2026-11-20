import { describe, expect, it } from 'vitest'
import { quantile } from './quantile'

describe('quantile', () => {
  it('interpolates inside the bucket that holds the rank', () => {
    expect(quantile(0.5, { '1': 10, '2': 10, '+Inf': 0 })).toBe(1)
    expect(quantile(0.75, { '1': 10, '2': 10, '+Inf': 0 })).toBe(1.5)
  })
  it('reports the open-ended bucket as its lower bound', () => {
    expect(quantile(0.99, { '1': 1, '2': 1, '+Inf': 8 })).toBe(2)
  })
  it('has no answer without counts', () => {
    expect(quantile(0.5, { '1': 0, '+Inf': 0 })).toBeNull()
  })
})
