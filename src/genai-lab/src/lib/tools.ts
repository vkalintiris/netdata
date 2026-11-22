// The Overview's table of tools, from the tool charts grouped by tool.
import { totals, type Series } from './agent'
import { quantile } from './quantile'

export const TOOL_LABEL = 'gen_ai.tool.name'

// What the Agent reports for a series without the label: a producer that does not name its tools.
export const UNNAMED_TOOL = '[unset]'

export type ToolRow = {
  name: string
  calls: number
  failed: number
  // The median duration; null without a recorded duration.
  typical: number | null
  // The median lies under the histogram's first bound, so `typical` is only an upper limit.
  typicalIsLimit: boolean
  slowest: number | null
}

// A series grouped by dimension and tool is labelled "<dimension>,<tool>"; a tool's name may hold commas.
function split(label: string): [string, string] {
  const at = label.indexOf(',')
  return at < 0 ? [label, ''] : [label.slice(0, at), label.slice(at + 1)]
}

// `calls` and `histogram` are grouped by dimension and tool, `max` by tool; each is one point over the window.
export function toolRows(calls: Series | undefined, histogram: Series | undefined, max: Series | undefined): ToolRow[] {
  if (!calls) return []
  const rows = new Map<string, ToolRow>()
  for (const [label, count] of Object.entries(totals(calls))) {
    const [outcome, name] = split(label)
    let row = rows.get(name)
    if (!row) {
      row = { name, calls: 0, failed: 0, typical: null, typicalIsLimit: false, slowest: null }
      rows.set(name, row)
    }
    row.calls += count
    if (outcome === 'error') row.failed += count
  }
  // A tool without a value has none recorded in the window: not a duration of zero.
  const slowest: Record<string, number | null> = {}
  max?.labels.forEach((name, i) => {
    const values = max.values[i].filter((v): v is number => v != null)
    slowest[name] = values.length ? Math.max(...values) : null
  })
  const buckets = new Map<string, Record<string, number>>()
  for (const [label, count] of Object.entries(histogram ? totals(histogram) : {})) {
    const [le, name] = split(label)
    buckets.set(name, { ...buckets.get(name), [le]: count })
  }
  for (const row of rows.values()) {
    row.slowest = slowest[row.name] ?? null
    const own = buckets.get(row.name)
    const median = own ? quantile(0.5, own) : null
    if (median == null) continue
    const firstBound = Math.min(...Object.keys(own!).map(Number).filter((le) => !Number.isNaN(le)))
    row.typicalIsLimit = median <= firstBound
    // Interpolation inside a bucket can land above the slowest call seen; the maximum bounds it.
    const limit = row.typicalIsLimit ? firstBound : median
    row.typical = row.slowest != null ? Math.min(limit, row.slowest) : limit
  }
  return [...rows.values()].filter((r) => r.calls > 0).sort((a, b) => b.calls - a.calls || a.name.localeCompare(b.name))
}
