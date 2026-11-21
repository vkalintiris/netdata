// The otel-logs Function: log records as flat field maps, for producers that send content as log events.
import { BusyError } from './traces'

export type LogRecord = Record<string, string>

type LogsAnswer = {
  columns?: Record<string, { index: number }>
  data?: (string | number | null)[][]
  status?: number
  errorMessage?: string
}

const PAGE = 1000
// A long session is a few thousand events; beyond this the page shows what it has rather than keep paging.
const MAX_RECORDS = 20_000

async function page(body: unknown, signal?: AbortSignal): Promise<LogsAnswer> {
  const res = await fetch('/api/v3/function?function=otel-logs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  })
  const json = (await res.json().catch(() => ({}))) as LogsAnswer
  if (res.status === 503) throw new BusyError('The Agent is busy')
  if (!res.ok || (typeof json.status === 'number' && json.status !== 200)) {
    throw new Error(json.errorMessage ?? `The Agent answered HTTP ${res.status}`)
  }
  return json
}

export function parseRows(a: LogsAnswer): LogRecord[] {
  const names: string[] = []
  for (const [name, c] of Object.entries(a.columns ?? {})) names[c.index] = name
  return (a.data ?? []).map((row) => {
    const out: LogRecord = {}
    row.forEach((v, i) => {
      if (v != null && names[i]) out[names[i]] = String(v)
    })
    return out
  })
}

// Every record in [after, before) matching the selections, newest first.
export async function fetchLogs(
  after: number,
  before: number,
  selections: Record<string, string[]>,
  signal?: AbortSignal,
): Promise<LogRecord[]> {
  const out: LogRecord[] = []
  let anchor: string | undefined
  while (out.length < MAX_RECORDS) {
    const rows = parseRows(
      await page({ after, before, last: PAGE, direction: 'backward', selections, ...(anchor ? { anchor } : {}) }, signal),
    )
    out.push(...rows)
    const next = rows.at(-1)?.cursor
    if (rows.length < PAGE || !next || next === anchor) break
    anchor = next
  }
  return out
}
