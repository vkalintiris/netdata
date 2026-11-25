// The otel-logs Function: log records as flat field maps, a page at a time, newest first.
import { BusyError } from './traces'

export type LogRecord = Record<string, string>

export type LogsAnswer = {
  columns?: Record<string, { index: number }>
  data?: (string | number | null)[][]
  items?: { matched?: number }
  status?: number
  errorMessage?: string
}

export type LogsPage = { records: LogRecord[]; matched: number; next: string | undefined }

export function parseRows(a: LogsAnswer): LogRecord[] {
  const names: string[] = []
  for (const [name, c] of Object.entries(a.columns ?? {})) names[c.index] = name
  return (a.data ?? []).map((row) => {
    const out: LogRecord = {}
    row.forEach((v, i) => {
      if (v != null && v !== '' && names[i]) out[names[i]] = String(v)
    })
    return out
  })
}

// One page of the records in [after, before) matching `selections` ({field: [allowed values]}), from `anchor` on.
export async function fetchLogsPage(
  window: { after: number; before: number },
  selections: Record<string, string[]>,
  anchor: string | undefined,
  limit: number,
  signal?: AbortSignal,
): Promise<LogsPage> {
  const body = { ...window, last: limit, direction: 'backward', selections, ...(anchor ? { anchor } : {}) }
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
  const records = parseRows(json)
  const last = records.at(-1)?.cursor
  return { records, matched: json.items?.matched ?? records.length, next: records.length < limit ? undefined : last }
}
