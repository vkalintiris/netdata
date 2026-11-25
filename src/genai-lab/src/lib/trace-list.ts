// GenAI spans grouped by trace: what one trace spent on model calls and tools, for the Traces page.
import { field, K, type SpanRow } from './traces'

export type TraceRow = {
  traceId: string
  service: string | undefined
  // First span's start and last span's end; the application's own spans around them are not read.
  startNs: number
  endNs: number
  // Time spent inside model calls; calls made in parallel add up past the wall time.
  inCallsNs: number
  // Model calls and tool calls, oldest first.
  spans: SpanRow[]
  calls: number
  failedCalls: number
  tools: number
  failedTools: number
  models: string[]
  providers: string[]
  tokensIn: number | null
  tokensOut: number | null
}

function add(sum: number | null, text: string | undefined): number | null {
  const v = text == null ? NaN : Number(text)
  if (Number.isNaN(v)) return sum
  return (sum ?? 0) + v
}

function push(list: string[], value: string | undefined) {
  if (value && !list.includes(value)) list.push(value)
}

export function isTool(row: SpanRow): boolean {
  return field(row.fields, K.kind) === 'tool'
}

// Traces, latest start first. Rows may come in any order and repeat across pages.
export function groupTraces(rows: SpanRow[]): TraceRow[] {
  const traces = new Map<string, TraceRow>()
  const seen = new Set<string>()
  for (const row of [...rows].sort((a, b) => a.start_ns - b.start_ns)) {
    const id = `${row.trace_id}:${row.span_id}`
    if (seen.has(id)) continue
    seen.add(id)

    let trace = traces.get(row.trace_id)
    if (!trace) {
      trace = {
        traceId: row.trace_id,
        service: field(row.fields, K.service),
        startNs: row.start_ns,
        endNs: row.start_ns + row.duration_ns,
        inCallsNs: 0,
        spans: [],
        calls: 0,
        failedCalls: 0,
        tools: 0,
        failedTools: 0,
        models: [],
        providers: [],
        tokensIn: null,
        tokensOut: null,
      }
      traces.set(row.trace_id, trace)
    }

    trace.spans.push(row)
    trace.endNs = Math.max(trace.endNs, row.start_ns + row.duration_ns)
    const failed = field(row.fields, K.outcome) === 'error'
    if (isTool(row)) {
      trace.tools += 1
      if (failed) trace.failedTools += 1
      continue
    }
    trace.calls += 1
    if (failed) trace.failedCalls += 1
    trace.inCallsNs += row.duration_ns
    push(trace.models, field(row.fields, K.requestModel))
    push(trace.providers, field(row.fields, K.provider))
    trace.tokensIn = add(trace.tokensIn, field(row.fields, K.inputTokens))
    trace.tokensOut = add(trace.tokensOut, field(row.fields, K.outputTokens))
  }
  return [...traces.values()].sort((a, b) => b.startNs - a.startNs)
}
