// Queries to the Netdata Agent's /api/v3/data, as the GenAI charts need them.

export type TimeWindow = { after: number; before: number }

export type Filters = Record<string, string[]>

export type Series = {
  labels: string[]
  times: number[]
  // values[i][j]: series i at times[j]; null where the Agent has no data
  values: (number | null)[][]
  units: string
  pointSeconds: number
  after: number
  before: number
}

export type DataQuery = {
  context: string
  window: TimeWindow
  points: number
  timeGroup: 'sum' | 'average' | 'max'
  aggregation: 'sum' | 'average' | 'max'
  groupByLabels?: string[]
  dimensions?: string[]
  filters?: Filters
}

export function scopeLabels(filters: Filters | undefined): string | undefined {
  if (!filters) return undefined
  const parts: string[] = []
  for (const [key, values] of Object.entries(filters)) {
    for (const value of values) parts.push(`${key}:${value}`)
  }
  return parts.length ? parts.join('|') : undefined
}

export function dataUrl(q: DataQuery): string {
  const p = new URLSearchParams()
  p.set('scope_contexts', q.context)
  p.set('after', String(q.window.after))
  p.set('before', String(q.window.before))
  p.set('points', String(q.points))
  p.set('time_group', q.timeGroup)
  p.set('aggregation', q.aggregation)
  p.set('format', 'json2')
  // Without it a one-point query is answered for a window aligned to its own length.
  p.set('options', 'unaligned')
  if (q.groupByLabels?.length) {
    p.set('group_by', 'label')
    p.set('group_by_label', q.groupByLabels.join(','))
  }
  if (q.dimensions?.length) p.set('dimensions', q.dimensions.join('|'))
  const labels = scopeLabels(q.filters)
  if (labels) p.set('scope_labels', labels)
  return `/api/v3/data?${p.toString()}`
}

type Json2 = {
  result?: { labels?: string[]; data?: [number, ...([number | null, number, number] | number)[]][] }
  view?: { units?: string | string[]; update_every?: number; after?: number; before?: number }
}

export function parseSeries(body: Json2): Series {
  const labels = (body.result?.labels ?? []).slice(1)
  const rows = [...(body.result?.data ?? [])].sort((a, b) => a[0] - b[0])
  const times: number[] = []
  const values: (number | null)[][] = labels.map(() => [])
  for (const row of rows) {
    times.push(row[0])
    for (let i = 0; i < labels.length; i++) {
      const cell = row[i + 1]
      const v = Array.isArray(cell) ? cell[0] : cell
      values[i].push(typeof v === 'number' ? v : null)
    }
  }
  const units = body.view?.units
  return {
    labels,
    times,
    values,
    units: Array.isArray(units) ? (units[0] ?? '') : (units ?? ''),
    pointSeconds: body.view?.update_every ?? 0,
    after: body.view?.after ?? 0,
    before: body.view?.before ?? 0,
  }
}

export class AgentError extends Error {}

export async function fetchSeries(q: DataQuery, signal?: AbortSignal): Promise<Series> {
  const res = await fetch(dataUrl(q), { signal })
  if (res.status === 401 || res.status === 403 || res.status === 412) {
    throw new AgentError(`The Agent refused the query (HTTP ${res.status}): mint a new bearer into .bearer`)
  }
  if (!res.ok) throw new AgentError(`The Agent answered HTTP ${res.status}`)
  return parseSeries(await res.json())
}

// Window totals: one point summed over the window, per label.
export function totals(s: Series): Record<string, number> {
  const out: Record<string, number> = {}
  s.labels.forEach((label, i) => {
    let sum = 0
    for (const v of s.values[i]) if (v != null) sum += v
    out[label] = sum
  })
  return out
}
