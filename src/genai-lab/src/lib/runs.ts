// Model calls grouped by trace: what one run of an application spent, for the Runs page.
import { field, K, type SpanRow } from './traces'

export type Run = {
  traceId: string
  service: string | undefined
  // First call's start and last call's end; the application's own spans around them are not read.
  startNs: number
  endNs: number
  // Time spent inside model calls; calls made in parallel add up past the wall time.
  inCallsNs: number
  // Oldest first.
  calls: SpanRow[]
  failed: number
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

// Runs, latest start first. Rows may come in any order and repeat across pages.
export function groupRuns(rows: SpanRow[]): Run[] {
  const runs = new Map<string, Run>()
  const seen = new Set<string>()
  for (const row of [...rows].sort((a, b) => a.start_ns - b.start_ns)) {
    const id = `${row.trace_id}:${row.span_id}`
    if (seen.has(id)) continue
    seen.add(id)

    let run = runs.get(row.trace_id)
    if (!run) {
      run = {
        traceId: row.trace_id,
        service: field(row.fields, K.service),
        startNs: row.start_ns,
        endNs: row.start_ns + row.duration_ns,
        inCallsNs: 0,
        calls: [],
        failed: 0,
        models: [],
        providers: [],
        tokensIn: null,
        tokensOut: null,
      }
      runs.set(row.trace_id, run)
    }

    run.calls.push(row)
    run.endNs = Math.max(run.endNs, row.start_ns + row.duration_ns)
    run.inCallsNs += row.duration_ns
    if (field(row.fields, K.outcome) === 'error') run.failed += 1
    push(run.models, field(row.fields, K.requestModel))
    push(run.providers, field(row.fields, K.provider))
    run.tokensIn = add(run.tokensIn, field(row.fields, K.inputTokens))
    run.tokensOut = add(run.tokensOut, field(row.fields, K.outputTokens))
  }
  return [...runs.values()].sort((a, b) => b.startNs - a.startNs)
}
