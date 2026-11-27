// The Breakdown page: model calls added up per value of one span attribute the user picks.
import type { Series } from './agent'
import { K, type ExploreAnswer } from './traces'

// The sums the page asks the Function for, in this order.
export const SUM_KEYS = [K.inputTokens, K.outputTokens, 'duration', K.cost] as const

export const MEASURES = {
  calls: 'Calls',
  failed: 'Failed calls',
  tokens: 'Tokens',
  tokensIn: 'Tokens in',
  tokensOut: 'Tokens out',
  time: 'Time in calls',
  cost: 'Cost',
} as const

export type Measure = keyof typeof MEASURES

export function isMeasure(v: string | null): v is Measure {
  return v != null && v in MEASURES
}

// `other` adds up the values past the Function's limit; `unset` the spans without the attribute.
export type RowKind = 'value' | 'other' | 'unset'

export type BreakdownRow = {
  kind: RowKind
  value: string
  calls: number
  failed: number
  // Null when the Agent answers no sums (one that predates them).
  tokensIn: number | null
  tokensOut: number | null
  seconds: number | null
  // US dollars, estimated; the calls counted in `unpriced` add nothing to it.
  usd: number | null
  // Calls that report tokens for a model the price table does not know.
  unpriced: number
}

export function measureOf(row: BreakdownRow, measure: Measure): number | null {
  switch (measure) {
    case 'calls':
      return row.calls
    case 'failed':
      return row.failed
    case 'tokensIn':
      return row.tokensIn
    case 'tokensOut':
      return row.tokensOut
    case 'tokens':
      return row.tokensIn == null || row.tokensOut == null ? null : row.tokensIn + row.tokensOut
    case 'time':
      return row.seconds
    case 'cost':
      return row.usd
  }
}

type Totals = { values: Map<string, number>; other: number; unset: number }

function counts(a: ExploreAnswer | undefined): Totals {
  const out: Totals = { values: new Map(), other: 0, unset: 0 }
  const h = a?.histogram
  if (!h) return out
  for (const d of h.dimensions) out.values.set(d, 0)
  for (const b of h.buckets) {
    h.dimensions.forEach((d, i) => out.values.set(d, (out.values.get(d) ?? 0) + (b.counts[i] ?? 0)))
    out.other += b.other
    out.unset += b.unset
  }
  return out
}

// The answer to a question nothing matches, over the time and the values of `a`: the failed calls of a page whose
// Outcome filter keeps only the succeeded.
export function noneOf(a: ExploreAnswer | undefined): ExploreAnswer | undefined {
  const h = a?.histogram
  if (!a || !h) return undefined
  const buckets = h.buckets.map(() => ({ counts: h.dimensions.map(() => 0), other: 0, unset: 0 }))
  return { ...a, histogram: { stack: h.stack, dimensions: h.dimensions, buckets }, rows: { matched: 0, items: [] } }
}

// The calls of `part` that belong to what `all` does not name: `part` is the same question asked of some of the
// calls, and may name values `all` folds into its `other`.
function unnamed(part: Totals, all: Totals): number {
  let named = 0
  for (const [value, n] of part.values) if (all.values.has(value)) named += n
  const total = [...part.values.values()].reduce((s, n) => s + n, 0) + part.other
  return Math.max(0, total - named)
}

// One row per value of the attribute, then what the Function could not name. `failures` and `unpriced` are the same
// question asked of the failed calls and of the calls without a known price; a value they do not name had none
// among its most frequent ones.
export function breakdownRows(
  all: ExploreAnswer | undefined,
  failures: ExploreAnswer | undefined,
  unpriced: ExploreAnswer | undefined,
): BreakdownRow[] {
  const h = all?.histogram
  if (!h) return []
  const total = counts(all)
  const failed = counts(failures)
  const noPrice = counts(unpriced)
  const sums = h.sums?.length === SUM_KEYS.length ? h.sums : null
  const sum = (s: number, at: number | 'other' | 'unset') => {
    if (!sums) return null
    const one = sums[s]
    return at === 'other' ? one.other : at === 'unset' ? one.unset : (one.totals[at] ?? 0)
  }
  const seconds = (ns: number | null) => (ns == null ? null : ns / 1e9)
  const usd = seconds

  const rows: BreakdownRow[] = h.dimensions.map((value, i) => ({
    kind: 'value',
    value,
    calls: total.values.get(value) ?? 0,
    failed: failed.values.get(value) ?? 0,
    tokensIn: sum(0, i),
    tokensOut: sum(1, i),
    seconds: seconds(sum(2, i)),
    usd: usd(sum(3, i)),
    unpriced: noPrice.values.get(value) ?? 0,
  }))
  if (total.other > 0) {
    rows.push({
      kind: 'other',
      value: 'other values',
      calls: total.other,
      failed: unnamed(failed, total),
      tokensIn: sum(0, 'other'),
      tokensOut: sum(1, 'other'),
      seconds: seconds(sum(2, 'other')),
      usd: usd(sum(3, 'other')),
      unpriced: unnamed(noPrice, total),
    })
  }
  if (total.unset > 0) {
    rows.push({
      kind: 'unset',
      value: 'not set',
      calls: total.unset,
      failed: failed.unset,
      tokensIn: sum(0, 'unset'),
      tokensOut: sum(1, 'unset'),
      seconds: seconds(sum(2, 'unset')),
      usd: usd(sum(3, 'unset')),
      unpriced: noPrice.unset,
    })
  }
  return rows
}

// Rows by the measure, largest first; what the Function could not name stays last.
export function ranked(rows: BreakdownRow[], measure: Measure): BreakdownRow[] {
  const by = (r: BreakdownRow) => measureOf(r, measure) ?? -1
  const named = rows.filter((r) => r.kind === 'value').sort((a, b) => by(b) - by(a) || a.value.localeCompare(b.value))
  return [...named, ...rows.filter((r) => r.kind !== 'value')]
}

// The measure over time for the values named in `shown`; every other value, named or not, adds up as `rest`. Calls
// without the attribute are not drawn: where few calls carry it, they would bury the ones that do.
export function breakdownSeries(
  all: ExploreAnswer | undefined,
  failures: ExploreAnswer | undefined,
  measure: Measure,
  shown: string[],
  rest: string,
): Series | undefined {
  const source = measure === 'failed' ? failures : all
  const h = source?.histogram
  if (!source || !h) return undefined
  const sums = h.sums?.length === SUM_KEYS.length
  if (!sums && measure !== 'calls' && measure !== 'failed') return undefined

  // Durations are in nanoseconds, costs in billionths of a dollar.
  const scale = measure === 'time' || measure === 'cost' ? 1e-9 : 1
  const which =
    measure === 'tokensIn' ? [0] : measure === 'tokensOut' ? [1] : measure === 'tokens' ? [0, 1] : measure === 'cost' ? [3] : [2]
  const byCount = measure === 'calls' || measure === 'failed'
  const index = shown.map((value) => h.dimensions.indexOf(value))
  const values: number[][] = [...shown.map(() => [] as number[]), []]

  for (const b of h.buckets) {
    let named = 0
    let total = 0
    const at = (i: number) => {
      if (byCount) return b.counts[i] ?? 0
      let v = 0
      for (const s of which) v += b.sums?.[s]?.values[i] ?? 0
      return v
    }
    h.dimensions.forEach((_, i) => (total += at(i)))
    if (byCount) total += b.other
    else for (const s of which) total += b.sums?.[s]?.other ?? 0
    index.forEach((i, row) => {
      const v = i < 0 ? 0 : at(i)
      named += v
      values[row].push(v * scale)
    })
    values[shown.length].push(Math.max(0, total - named) * scale)
  }

  const w = source.window
  return {
    labels: [...shown, rest],
    times: h.buckets.map((_, j) => w.bucket_start_s + j * w.bucket_width_s),
    values,
    units: measure === 'time' ? 'seconds' : measure === 'cost' ? 'USD' : '',
    pointSeconds: w.bucket_width_s,
    after: w.after,
    before: w.before,
  }
}
