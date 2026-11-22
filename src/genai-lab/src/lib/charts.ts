// Chart pieces the Overview's sections share.
import type { SeriesStyle } from '@/components/TimeChart'
import type { Series } from './agent'
import { quantile } from './quantile'

export const OUTCOME_STYLES: SeriesStyle[] = [
  { label: 'success', name: 'Success', color: '--success' },
  { label: 'error', name: 'Error', color: '--error' },
]

export const LATENCY_STYLES: SeriesStyle[] = [
  { label: 'p50', name: 'p50', color: '--chart-1' },
  { label: 'p95', name: 'p95', color: '--chart-3' },
  { label: 'maximum', name: 'Maximum', color: '--chart-4' },
]

// Percentiles per point from the histogram's bucket counts, which add up across series; the maximum comes from its
// own chart. (A mean of per-series averages would weigh a rarely used model like a busy one.)
export function latencyLines(hist: Series | undefined, max: Series | undefined): Series | undefined {
  if (!hist || !max) return undefined
  const maxAt = new Map(max.times.map((t, i) => [t, max.values[0]?.[i] ?? null]))
  const p50: (number | null)[] = []
  const p95: (number | null)[] = []
  // Interpolation inside a wide bucket can land above the slowest call seen; the maximum bounds it.
  const bounded = (q: number | null, m: number | null | undefined) => (q != null && m != null ? Math.min(q, m) : q)
  hist.times.forEach((t, j) => {
    const buckets: Record<string, number> = {}
    hist.labels.forEach((le, i) => (buckets[le] = hist.values[i][j] ?? 0))
    p50.push(bounded(quantile(0.5, buckets), maxAt.get(t)))
    p95.push(bounded(quantile(0.95, buckets), maxAt.get(t)))
  })
  return {
    ...hist,
    labels: ['p50', 'p95', 'maximum'],
    values: [p50, p95, hist.times.map((t) => maxAt.get(t) ?? null)],
    units: 'seconds',
  }
}
