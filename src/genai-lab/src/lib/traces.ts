// The otel-traces Function: model-call spans for the Calls page and whole traces for the call detail.
import type { Filters, TimeWindow } from './agent'
import { absolute, LABELS } from './page-state'

export const K = {
  service: 'resource.attributes.service.name',
  name: 'name',
  status: 'status.code',
  statusMessage: 'status.message',
  operation: 'attributes.gen_ai.operation.name',
  requestModel: 'attributes.gen_ai.request.model',
  responseModel: 'attributes.gen_ai.response.model',
  provider: 'attributes.gen_ai.provider.name',
  system: 'attributes.gen_ai.system',
  inputTokens: 'attributes.gen_ai.usage.input_tokens',
  outputTokens: 'attributes.gen_ai.usage.output_tokens',
  cacheRead: 'attributes.gen_ai.usage.cache_read.input_tokens',
  cacheWrite: 'attributes.gen_ai.usage.cache_creation.input_tokens',
  reasoning: 'attributes.gen_ai.usage.reasoning.output_tokens',
  finishReasons: 'attributes.gen_ai.response.finish_reasons',
  errorType: 'attributes.error.type',
} as const

export const MODEL_CALL_OPERATIONS = ['chat', 'text_completion', 'generate_content', 'embeddings']

const ROW_COLUMNS = [
  K.service,
  K.name,
  K.status,
  K.operation,
  K.requestModel,
  K.responseModel,
  K.provider,
  K.system,
  K.inputTokens,
  K.outputTokens,
  K.errorType,
]

// Page filters that have a span attribute to match; the provider has two attributes across producers, so it is
// not applied to the list (an exact match on one of them would drop the spans that carry the other).
const SPAN_KEYS: Record<string, string> = {
  [LABELS.service]: K.service,
  [LABELS.model]: K.requestModel,
}

export function unappliedFilters(filters: Filters): string[] {
  return Object.keys(filters).filter((k) => !(k in SPAN_KEYS))
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
  histogram?: { stack: string; dimensions: string[]; buckets: { counts: number[]; other: number; unset: number }[] }
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

async function post<T>(body: unknown, signal?: AbortSignal): Promise<T> {
  const res = await fetch('/api/v3/function?function=otel-traces', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  })
  const json = await res.json().catch(() => ({}))
  if (!res.ok || (json.status && json.status !== 200 && typeof json.status === 'number')) {
    throw new Error(json.errorMessage ?? `The Agent answered HTTP ${res.status}`)
  }
  return json as T
}

function conditions(filters: Filters) {
  const out: { key: string; op: string; values: string[] }[] = [
    { key: K.operation, op: '=', values: MODEL_CALL_OPERATIONS },
  ]
  for (const [label, values] of Object.entries(filters)) {
    const key = SPAN_KEYS[label]
    if (key && values.length) out.push({ key, op: '=', values })
  }
  return out
}

export const ROWS_PAGE = 100

export function exploreCalls(
  w: TimeWindow,
  filters: Filters,
  anchor: string | undefined,
  signal?: AbortSignal,
): Promise<ExploreAnswer> {
  const { after, before } = absolute(w)
  const rows = { limit: ROWS_PAGE, columns: ROW_COLUMNS }
  const body = anchor
    ? { explore: { conditions: conditions(filters), anchor, rows } }
    : { explore: { after, before, conditions: conditions(filters), histogram: { stack: K.requestModel }, rows } }
  return post<ExploreAnswer>(body, signal)
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
  const raw = await post<RawTrace>({ trace: { id, after: at - 3600, before: at + 3600 } }, signal)
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
