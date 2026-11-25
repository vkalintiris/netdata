import { useCallback, useMemo } from 'react'
import { useSearchParams } from 'react-router'
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

  // One URL update per clear: react-router does not chain functional updates made in the same event.
  const clearFilter = useCallback((name: FilterName | 'outcome' | 'kind') => update((p) => p.delete(name)), [update])

  const selected = useCallback((name: FilterName) => params.getAll(name), [params])

  return { window, filters, outcomes, kinds, setWindow, toggleFilter, toggleOutcome, toggleKind, clearFilter, selected }
}

// Absolute bounds of a window, for panning and zooming relative presets.
export function absolute(w: TimeWindow, now = Math.floor(Date.now() / 1000)): TimeWindow {
  if (w.after > 0) return w
  const before = w.before > 0 ? w.before : now + w.before
  return { after: before + w.after, before }
}
