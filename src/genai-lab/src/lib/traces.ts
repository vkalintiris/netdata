// The otel-traces Function: the GenAI spans the plugin recognised (model calls and tool calls) for the Spans and
// Traces pages, and whole traces for the span detail.
import type { TimeWindow } from './agent'
import { absolute, KINDS, LABELS, OUTCOMES, type SpanQuery } from './page-state'

// Span fields. The `netdata.genai.*` ones are what the plugin's GenAI rules made of the span when it arrived: the
// same values the charts count, whatever conventions the producer speaks.
export const K = {
  service: 'resource.attributes.service.name',
  name: 'name',
  status: 'status.code',
  statusMessage: 'status.message',
  kind: 'attributes.netdata.genai.kind',
  call: 'attributes.netdata.genai.call',
  tool: 'attributes.netdata.genai.tool',
  operation: 'attributes.netdata.genai.operation',
  requestModel: 'attributes.netdata.genai.request_model',
  responseModel: 'attributes.netdata.genai.response_model',
  provider: 'attributes.netdata.genai.provider',
  outcome: 'attributes.netdata.genai.outcome',
  inputTokens: 'attributes.netdata.genai.input_tokens',
  outputTokens: 'attributes.netdata.genai.output_tokens',
  cacheRead: 'attributes.netdata.genai.cache_read_tokens',
  cacheWrite: 'attributes.netdata.genai.cache_write_tokens',
  reasoning: 'attributes.netdata.genai.reasoning_tokens',
  finishReasons: 'attributes.gen_ai.response.finish_reasons',
  errorType: 'attributes.error.type',
} as const

// Model-call spans the charts do not count: the second span of a call another span counts, and a gateway's span of a
// call it forwarded (the Gateways pages show those). The pages list each call once.
export const NOT_COUNTED = ['duplicate', 'gateway']

const ROW_COLUMNS = [
  K.service,
  K.name,
  K.kind,
  K.tool,
  K.requestModel,
  K.provider,
  K.outcome,
  K.inputTokens,
  K.outputTokens,
  K.errorType,
]

// Page filters as span fields.
const SPAN_KEYS: Record<string, string> = {
  [LABELS.service]: K.service,
  [LABELS.model]: K.requestModel,
  [LABELS.provider]: K.provider,
}

export type Fields = [string, string][]

export type SpanRow = {
  start_ns: number
  duration_ns: number
  trace_id: string
  span_id: string
  fields: Fields
}

export type ExploreAnswer = {
  window: { after: number; before: number; bucket_start_s: number; bucket_width_s: number; buckets: number }
  histogram?: {
    stack: string
    dimensions: string[]
    buckets: {
      counts: number[]
      other: number
      unset: number
      durations?: number[]
      // One entry per sum asked for; `values` follows `dimensions`.
      sums?: { values: number[]; other: number; unset: number }[]
    }[]
    // One entry per sum asked for, in the order asked; `totals` follows `dimensions`. Absent when none was asked,
    // or from an Agent that predates sums.
    sums?: { key: string; totals: number[]; other: number; unset: number; total: number; skipped: number }[]
    // Upper bound of each duration row in ns; null for the open-ended last row.
    duration_rows?: (number | null)[]
  }
  rows: { matched: number; items: SpanRow[]; anchor?: { next?: string } }
  status?: { complete?: boolean }
}

export type TraceSpan = {
  span_id: string
  // Nesting level in the trace tree; spans come in tree order (parent, then its children).
  depth?: number
  start_ns: number
  duration_ns: number
  fields: Fields
  events?: { name?: string; time_unix_nano?: number; attributes?: Fields }[]
}

export type TraceAnswer = { trace_id: string; spans: TraceSpan[] }

export function field(fields: Fields | undefined, ...keys: string[]): string | undefined {
  if (!fields) return undefined
  for (const key of keys) {
    const hit = fields.find(([k]) => k === key)
    if (hit && hit[1] !== '') return hit[1]
  }
  return undefined
}

export class BusyError extends Error {}

// The Function answers an explore that overlaps another with `partial: ["busy"]` and no data, so this page sends its
// requests one at a time; a busy answer that still slips through is an error the caller retries, never data.
let queue: Promise<unknown> = Promise.resolve()

export function postTraces<T>(body: unknown, signal?: AbortSignal): Promise<T> {
  const run = async () => {
    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    const res = await fetch('/api/v3/function?function=otel-traces', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal,
    })
    const json = await res.json().catch(() => ({}))
    if (json?.status?.partial?.includes?.('busy') || res.status === 503) throw new BusyError('The Agent is busy')
    if (!res.ok || (typeof json.status === 'number' && json.status !== 200)) {
      throw new Error(json.errorMessage ?? `The Agent answered HTTP ${res.status}`)
    }
    return json as T
  }
  const next = queue.then(run, run)
  queue = next.catch(() => undefined)
  return next
}

export type Condition = { key: string; op: string; values: (string | number)[] }

export function conditions({ filters, outcomes, kinds, where, cell }: SpanQuery): Condition[] {
  const out: Condition[] = [
    { key: K.kind, op: '=', values: kinds.map((k) => KINDS[k]) },
    { key: K.call, op: '!=', values: NOT_COUNTED },
  ]
  for (const [label, values] of Object.entries(filters)) {
    const key = SPAN_KEYS[label]
    if (key && values.length) out.push({ key, op: '=', values })
  }
  if (outcomes.length) out.push({ key: K.outcome, op: '=', values: outcomes.map((o) => OUTCOMES[o]) })
  for (const [key, values] of Object.entries(where)) out.push({ key, op: '=', values })
  if (cell) {
    out.push({ key: 'duration', op: '>=', values: [Math.round(cell.min * 1e9)] })
    if (cell.max != null) out.push({ key: 'duration', op: '<', values: [Math.round(cell.max * 1e9)] })
  }
  return out
}

// The time range every request of a query covers: the page's window, or the cell the query is narrowed to.
function range(w: TimeWindow, query: SpanQuery): { after: number; before: number } {
  return query.cell ?? absolute(w)
}

export const ROWS_PAGE = 100

export function exploreSpans(
  w: TimeWindow,
  query: SpanQuery,
  anchor: string | undefined,
  signal?: AbortSignal,
  rowLimit = ROWS_PAGE,
): Promise<ExploreAnswer> {
  const { after, before } = range(w, query)
  const rows = { limit: rowLimit, columns: ROW_COLUMNS }
  const where = conditions(query)
  const body = anchor
    ? { explore: { conditions: where, anchor, rows } }
    : { explore: { after, before, conditions: where, histogram: { stack: K.requestModel }, rows } }
  return postTraces<ExploreAnswer>(body, signal)
}

type RawTrace = {
  trace_id: string
  roots?: number[]
  children?: number[][]
  resources?: { attributes?: Fields }[]
  spans?: (TraceSpan & { resource?: number })[]
}

export async function fetchTrace(id: string, aroundNs: number, signal?: AbortSignal): Promise<TraceAnswer> {
  const at = Math.floor(aroundNs / 1e9)
  const raw = await postTraces<RawTrace>({ trace: { id, after: at - 3600, before: at + 3600 } }, signal)
  const resources = raw.resources ?? []
  const spans = (raw.spans ?? []).map((s) => {
    const res = typeof s.resource === 'number' ? (resources[s.resource]?.attributes ?? []) : []
    return { ...s, fields: [...res, ...(s.fields ?? [])] }
  })
  const children = raw.children ?? []
  const ordered: TraceSpan[] = []
  const seen = new Set<number>()
  const walk = (i: number, depth: number) => {
    if (seen.has(i) || !spans[i]) return
    seen.add(i)
    ordered.push({ ...spans[i], depth })
    const kids = [...(children[i] ?? [])].sort((a, b) => spans[a].start_ns - spans[b].start_ns)
    for (const k of kids) walk(k, depth + 1)
  }
  for (const r of raw.roots ?? []) walk(r, 0)
  // Spans whose parent is missing from the answer still show, at the top level.
  spans.forEach((_, i) => walk(i, 0))
  return { trace_id: raw.trace_id, spans: ordered }
}

// Spans per time bucket and per value of `key`, with `sums` of numeric fields per value when asked; no rows. The
// window is the whole request: nothing is paged.
export function exploreGroups(
  w: TimeWindow,
  query: SpanQuery,
  key: string,
  sums: readonly string[],
  signal?: AbortSignal,
): Promise<ExploreAnswer> {
  const { after, before } = range(w, query)
  const histogram = sums.length ? { stack: key, sums } : { stack: key }
  return postTraces<ExploreAnswer>(
    { explore: { after, before, conditions: conditions(query), histogram, rows: { limit: 1, columns: [key] } } },
    signal,
  )
}

export type KeyChoice = { text: string; tier: string; kinds: string[] }

// Every attribute key the Function can group by in the window: the ones few enough in values to be indexed by
// value. A key with more values than that is not offered; grouping by it would answer one `other`.
export async function groupableKeys(w: TimeWindow, signal?: AbortSignal): Promise<KeyChoice[]> {
  const { after, before } = absolute(w)
  const out: KeyChoice[] = []
  for (const tier of ['low', 'mid']) {
    const a = await postTraces<{ items?: KeyChoice[] }>({ suggest: { after, before, tier } }, signal)
    out.push(...(a.items ?? []))
  }
  return out
}

// Spans per value of one attribute in a histogram answer; `other` holds the values past the Function's
// stack limit and `unset` the spans without the attribute.
export type StackTotals = { values: Record<string, number>; other: number; unset: number }

function stackTotals(a: ExploreAnswer | undefined): StackTotals | undefined {
  const h = a?.histogram
  if (!h) return undefined
  const values: Record<string, number> = {}
  h.dimensions.forEach((d, i) => {
    if (d) values[d] = h.buckets.reduce((sum, b) => sum + (b.counts[i] ?? 0), 0)
  })
  return {
    values,
    other: h.buckets.reduce((sum, b) => sum + b.other, 0),
    unset: h.buckets.reduce((sum, b) => sum + b.unset, 0),
  }
}

// GenAI spans per value of one span attribute: the filter choices of the Spans and Traces pages, which must
// include services whose metrics are older than the Agent keeps.
export async function spanStack(
  w: TimeWindow,
  query: SpanQuery,
  key: string,
  signal?: AbortSignal,
): Promise<StackTotals> {
  const { after, before } = range(w, query)
  const a = await postTraces<ExploreAnswer>(
    {
      explore: {
        after,
        before,
        conditions: conditions(query),
        histogram: { stack: key },
        rows: { limit: 1, columns: [key] },
      },
    },
    signal,
  )
  return stackTotals(a) ?? { values: {}, other: 0, unset: 0 }
}

export const SPAN_KEY_OF: Record<string, string> = SPAN_KEYS
