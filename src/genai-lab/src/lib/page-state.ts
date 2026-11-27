import { useCallback, useMemo } from 'react'
import { useLocation, useSearchParams } from 'react-router'
import type { Filters, TimeWindow } from './agent'

export const LABELS = {
  service: 'resource.attributes.service.name',
  model: 'gen_ai.request.model',
  provider: 'gen_ai.provider.name',
} as const

export type FilterName = keyof typeof LABELS

// The outcome filter matches the outcome the plugin stored on the span; the Metrics page's charts carry it as a
// dimension, not a label, so it applies on the Spans and Traces pages only.
export const OUTCOMES = { succeeded: 'success', failed: 'error' } as const

export type Outcome = keyof typeof OUTCOMES

function isOutcome(v: string): v is Outcome {
  return v in OUTCOMES
}

// What the plugin recognised a span as, as it stored it on the span.
export const KINDS = { 'model call': 'call', 'tool call': 'tool' } as const

export type Kind = keyof typeof KINDS

function isKind(v: string): v is Kind {
  return v in KINDS
}

export const ALL_KINDS = Object.keys(KINDS) as Kind[]

// What an empty Kind filter means on a tab: the kinds its rows are made of. Set here once, so every count on a tab
// (the header, the Outcome panel, the page itself) is of the rows that tab shows. Breakdown adds up tokens and
// failures, facts of model calls; the Traces lists show tool calls too.
const DEFAULT_KINDS: Record<string, Kind[]> = { '/breakdown': ['model call'] }

export function defaultKinds(pathname: string): Kind[] {
  return DEFAULT_KINDS[pathname] ?? ALL_KINDS
}

// What the counts of a query are counts of.
export function unitOf(kinds: Kind[]): string {
  return kinds.length === 1 ? `${kinds[0]}s` : 'model and tool calls'
}

// A heatmap cell the Traces tab is narrowed to: its time range in seconds and its duration range in seconds.
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

// Conditions on any span field, as the breakdown hands them over: field key to the values it may have.
export type Where = Record<string, string[]>

// What the span pages ask the traces Function for, besides the time window. `kinds` is never empty: an empty Kind
// filter has become the tab's default. A `cell` replaces the window with its own time range.
export type SpanQuery = { filters: Filters; outcomes: Outcome[]; kinds: Kind[]; where: Where; cell: CallCell | null }

function parseWhere(entries: string[]): Where {
  const out: Where = {}
  for (const entry of entries) {
    const at = entry.indexOf('=')
    if (at <= 0) continue
    const key = entry.slice(0, at)
    ;(out[key] ??= []).push(entry.slice(at + 1))
  }
  return out
}

export const PRESETS = [
  { label: '15m', after: -900 },
  { label: '1h', after: -3600 },
  { label: '6h', after: -6 * 3600 },
  { label: '24h', after: -24 * 3600 },
]

const DEFAULT_AFTER = -3600

// Points per trend chart. A fixed window is stretched to points × a whole step, which the Agent answers as asked;
// any other length it widens to whole steps, and the charts would then cover more time than the tiles.
export const POINTS = 120

export function snap(w: TimeWindow): TimeWindow {
  if (w.after <= 0) return w
  const step = Math.max(1, Math.ceil((w.before - w.after) / POINTS))
  const before = Math.round(w.before)
  return { after: before - step * POINTS, before }
}

// The relative window the page last showed, so a reset returns to it rather than to a fixed default.
let lastRelative = DEFAULT_AFTER

export function resetWindow(): TimeWindow {
  return { after: lastRelative, before: 0 }
}

// The time window and the filters live in the URL, so a view can be reloaded and shared.
export function usePageState() {
  const [params, setParams] = useSearchParams()
  const { pathname } = useLocation()

  const window: TimeWindow = useMemo(() => {
    const after = Number(params.get('after') ?? DEFAULT_AFTER)
    const before = Number(params.get('before') ?? 0)
    return {
      after: Number.isFinite(after) ? after : DEFAULT_AFTER,
      before: Number.isFinite(before) ? before : 0,
    }
  }, [params])

  const filters: Filters = useMemo(() => {
    const out: Filters = {}
    for (const [name, key] of Object.entries(LABELS)) {
      const values = params.getAll(name)
      if (values.length) out[key] = values
    }
    return out
  }, [params])

  const outcomes: Outcome[] = useMemo(() => params.getAll('outcome').filter(isOutcome), [params])
  const kinds: Kind[] = useMemo(() => params.getAll('kind').filter(isKind), [params])
  const where: Where = useMemo(() => parseWhere(params.getAll('where')), [params])
  const cellText = params.get('cell')
  const cell = useMemo(() => parseCell(cellText), [cellText])
  const spanQuery: SpanQuery = useMemo(
    () => ({ filters, outcomes, kinds: kinds.length ? kinds : defaultKinds(pathname), where, cell }),
    [filters, outcomes, kinds, where, cell, pathname],
  )

  const update = useCallback(
    (fn: (p: URLSearchParams) => void) =>
      setParams((prev) => {
        const next = new URLSearchParams(prev)
        fn(next)
        return next
      }),
    [setParams],
  )

  if (window.after < 0 && window.before === 0) lastRelative = window.after

  const setWindow = useCallback(
    (requested: TimeWindow) =>
      update((p) => {
        const w = snap(requested)
        p.set('after', String(Math.round(w.after)))
        if (w.before) p.set('before', String(Math.round(w.before)))
        else p.delete('before')
      }),
    [update],
  )

  const toggleFilter = useCallback(
    (name: FilterName, value: string) =>
      update((p) => {
        const values = p.getAll(name)
        p.delete(name)
        const next = values.includes(value) ? values.filter((v) => v !== value) : [...values, value]
        for (const v of next) p.append(name, v)
      }),
    [update],
  )

  const toggleChoice = useCallback(
    (name: 'outcome' | 'kind', value: string) =>
      update((p) => {
        const values = p.getAll(name)
        p.delete(name)
        for (const v of values.includes(value) ? values.filter((v) => v !== value) : [...values, value]) {
          p.append(name, v)
        }
      }),
    [update],
  )
  const toggleOutcome = useCallback((value: Outcome) => toggleChoice('outcome', value), [toggleChoice])
  const toggleKind = useCallback((value: Kind) => toggleChoice('kind', value), [toggleChoice])

  const removeWhere = useCallback(
    (key: string, value: string) =>
      update((p) => {
        const rest = p.getAll('where').filter((entry) => entry !== `${key}=${value}`)
        p.delete('where')
        for (const entry of rest) p.append('where', entry)
      }),
    [update],
  )

  // One URL update per clear: react-router does not chain functional updates made in the same event.
  const clearFilter = useCallback((name: FilterName | 'outcome' | 'kind') => update((p) => p.delete(name)), [update])

  const selected = useCallback((name: FilterName) => params.getAll(name), [params])

  return {
    window,
    filters,
    outcomes,
    kinds,
    where,
    spanQuery,
    setWindow,
    toggleFilter,
    toggleOutcome,
    toggleKind,
    removeWhere,
    clearFilter,
    selected,
  }
}

// What a time chart needs to move the page's window: a drag pans, a selection zooms, a double-click returns to the
// last relative window. A pan that reaches now turns the window relative again.
export function useChartNav() {
  const { window, setWindow } = usePageState()
  const onZoom = useCallback((after: number, before: number) => setWindow({ after, before }), [setWindow])
  const onPan = useCallback(
    (seconds: number) => {
      const w = absolute(window)
      const now = Math.floor(Date.now() / 1000)
      const shift = Math.round(seconds)
      if (w.before + shift >= now) setWindow({ after: -(w.before - w.after), before: 0 })
      else setWindow({ after: w.after + shift, before: w.before + shift })
    },
    [window, setWindow],
  )
  const onReset = useCallback(() => setWindow(resetWindow()), [setWindow])
  return useMemo(() => ({ onZoom, onPan, onReset }), [onZoom, onPan, onReset])
}

// Absolute bounds of a window, for panning and zooming relative presets.
export function absolute(w: TimeWindow, now = Math.floor(Date.now() / 1000)): TimeWindow {
  if (w.after > 0) return w
  const before = w.before > 0 ? w.before : now + w.before
  return { after: before + w.after, before }
}
