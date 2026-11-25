// The otel-traces Function: model-call spans for the Calls page and whole traces for the call detail.
import type { Filters, TimeWindow } from './agent'
import { absolute, LABELS, OUTCOMES, type Outcome } from './page-state'

// Span fields. The `netdata.genai.*` ones are what the plugin's GenAI rules made of the span when it arrived: the
// same values the charts count, whatever conventions the producer speaks.
export const K = {
  service: 'resource.attributes.service.name',
  name: 'name',
  status: 'status.code',
  statusMessage: 'status.message',
  call: 'attributes.netdata.genai.call',
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

// The span a model call is counted by on the charts; a second span of the same call is marked `duplicate`.
const COUNTED = 'counted'

const ROW_COLUMNS = [
  K.service,
  K.name,
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
    buckets: { counts: number[]; other: number; unset: number; durations?: number[] }[]
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
  events?: { name?: string; time_ns?: number; fields?: Fields }[]
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

// A heatmap cell the list is narrowed to: its time range in seconds and its duration range in seconds.
export type CallCell = { after: number; before: number; min: number; max: number | null }

export function parseCell(text: string | null): CallCell | null {
  if (!text) return null
  const [after, before, min, max] = text.split(':').map(Number)
  if (![after, before, min].every(Number.isFinite)) return null
  return { after, before, min, max: Number.isFinite(max) ? max : null }
}

export function cellParam(c: CallCell): string {
  return [c.after, c.before, c.min, c.max ?? ''].join(':')
}

function conditions(filters: Filters, outcomes: Outcome[], cell?: CallCell | null) {
  const out: { key: string; op: string; values: (string | number)[] }[] = [
    { key: K.call, op: '=', values: [COUNTED] },
  ]
  for (const [label, values] of Object.entries(filters)) {
    const key = SPAN_KEYS[label]
    if (key && values.length) out.push({ key, op: '=', values })
  }
  if (outcomes.length) out.push({ key: K.outcome, op: '=', values: outcomes.map((o) => OUTCOMES[o]) })
  if (cell) {
    out.push({ key: 'duration', op: '>=', values: [Math.round(cell.min * 1e9)] })
    if (cell.max != null) out.push({ key: 'duration', op: '<', values: [Math.round(cell.max * 1e9)] })
  }
  return out
}

export const ROWS_PAGE = 100

export function exploreCalls(
  w: TimeWindow,
  filters: Filters,
  outcomes: Outcome[],
  anchor: string | undefined,
  signal?: AbortSignal,
  cell?: CallCell | null,
  rowLimit = ROWS_PAGE,
): Promise<ExploreAnswer> {
  const { after, before } = cell ?? absolute(w)
  const rows = { limit: rowLimit, columns: ROW_COLUMNS }
  const where = conditions(filters, outcomes, cell)
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

// Model-call spans per value of one attribute in a histogram answer; `other` holds the values past the Function's
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

// Model-call spans per value of one span attribute: the Calls page's breakdowns and filter choices, which must
// include services whose metrics are older than the Agent keeps.
export async function spanStack(
  w: TimeWindow,
  filters: Filters,
  outcomes: Outcome[],
  key: string,
  signal?: AbortSignal,
): Promise<StackTotals> {
  const { after, before } = absolute(w)
  const a = await postTraces<ExploreAnswer>(
    {
      explore: {
        after,
        before,
        conditions: conditions(filters, outcomes),
        histogram: { stack: key },
        rows: { limit: 1, columns: [key] },
      },
    },
    signal,
  )
  return stackTotals(a) ?? { values: {}, other: 0, unset: 0 }
}

export const SPAN_KEY_OF: Record<string, string> = SPAN_KEYS
