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
  // With `groupByLabels`: one series per dimension and label value, labelled "<dimension>,<label values>".
  groupByDimension?: boolean
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

// Points per trend chart. A fixed window is stretched to points × a whole step, which the Agent answers as asked;
// any other length it widens to whole steps, and the charts would then cover more time than the tiles.
export const POINTS = 120

// A trend over a window that follows "now", as whole steps that start on multiples of the step: a point covers the
// same seconds in every request, so a refresh adds points at the end and leaves the earlier ones as they were.
// Without this the Agent cuts the points back from the moment of each request, and the same data draws differently
// every time. A one-point query (a total) covers the seconds of its page's trends, so a tile is the sum of the chart
// under it. A fixed window is asked as it is.
export function clockAligned(q: Pick<DataQuery, 'window' | 'points'>, now: number): { window: TimeWindow; points: number } {
  const { window, points } = q
  if (window.after >= 0) return { window, points }
  const total = points <= 1
  const end = window.before > 0 ? window.before : now + window.before
  const step = Math.max(1, Math.ceil(-window.after / (total ? POINTS : points)))
  // A window that reaches the current second is answered for the second before, which moves every point: end on
  // the last step boundary that is at least two seconds old.
  const before = Math.floor((end - 2) / step) * step
  const whole = Math.max(1, Math.floor(-window.after / step))
  return { window: { after: before - whole * step, before }, points: total ? 1 : whole }
}

export function dataUrl(q: DataQuery, now = Math.floor(Date.now() / 1000)): string {
  const { window, points } = clockAligned(q, now)
  const p = new URLSearchParams()
  p.set('scope_contexts', q.context)
  p.set('after', String(window.after))
  p.set('before', String(window.before))
  p.set('points', String(points))
  p.set('time_group', q.timeGroup)
  p.set('aggregation', q.aggregation)
  p.set('format', 'json2')
  // Without it a one-point query is answered for a window aligned to its own length.
  p.set('options', 'unaligned')
  // Left to itself the Agent reads a total and a trend of one window from different storage tiers, and the coarser
  // ones add these sparse counts up short: a tile then disagrees with the chart under it.
  p.set('tier', '0')
  if (q.groupByLabels?.length) {
    p.set('group_by', q.groupByDimension ? 'label,dimension' : 'label')
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
