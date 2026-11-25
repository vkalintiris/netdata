// Chart pieces the pages share: latency lines, quantiles and totals from histogram charts, gauge readings.
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

// Per-bucket histogram charts (one dimension per upper bound) as one quantile line each, point by point.
export function quantileLines(q: number, hists: { label: string; series: Series | undefined }[]): Series | undefined {
  const first = hists.find((h) => h.series)?.series
  if (!first) return undefined
  const values = hists.map(({ series }) =>
    first.times.map((t) => {
      if (!series) return null
      const j = series.times.indexOf(t)
      if (j < 0) return null
      const buckets: Record<string, number> = {}
      series.labels.forEach((le, i) => (buckets[le] = series.values[i][j] ?? 0))
      return quantile(q, buckets)
    }),
  )
  return { ...first, labels: hists.map((h) => h.label), values, units: 'seconds' }
}

// Window total of a per-bucket histogram chart, as buckets.
export function bucketTotals(s: Series | undefined): Record<string, number> | undefined {
  if (!s) return undefined
  const out: Record<string, number> = {}
  s.labels.forEach((le, i) => (out[le] = s.values[i].reduce<number>((sum, v) => sum + (v ?? 0), 0)))
  return out
}

// The newest value of each series: the current reading of a gauge.
export function latest(s: Series | undefined): Record<string, number> {
  const out: Record<string, number> = {}
  if (!s) return out
  s.labels.forEach((label, i) => {
    for (let j = s.values[i].length - 1; j >= 0; j--) {
      const v = s.values[i][j]
      if (v != null) {
        out[label] = v
        return
      }
    }
  })
  return out
}
