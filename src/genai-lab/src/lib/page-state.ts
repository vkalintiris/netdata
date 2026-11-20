import { useCallback, useMemo } from 'react'
import { useSearchParams } from 'react-router'
import type { Filters, TimeWindow } from './agent'

export const LABELS = {
  service: 'resource.attributes.service.name',
  model: 'gen_ai.request.model',
  provider: 'gen_ai.provider.name',
} as const

export type FilterName = keyof typeof LABELS

export const PRESETS = [
  { label: '15m', after: -900 },
  { label: '1h', after: -3600 },
  { label: '6h', after: -6 * 3600 },
  { label: '24h', after: -24 * 3600 },
]

const DEFAULT_AFTER = -3600

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

  const update = useCallback(
    (fn: (p: URLSearchParams) => void) =>
      setParams((prev) => {
        const next = new URLSearchParams(prev)
        fn(next)
        return next
      }),
    [setParams],
  )

  const setWindow = useCallback(
    (w: TimeWindow) =>
      update((p) => {
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

  const clearFilters = useCallback(
    () =>
      update((p) => {
        for (const name of Object.keys(LABELS)) p.delete(name)
      }),
    [update],
  )

  const selected = useCallback((name: FilterName) => params.getAll(name), [params])

  return { window, filters, setWindow, toggleFilter, clearFilters, selected, search: params.toString() }
}

// Absolute bounds of a window, for panning and zooming relative presets.
export function absolute(w: TimeWindow, now = Math.floor(Date.now() / 1000)): TimeWindow {
  if (w.after > 0) return w
  const before = w.before > 0 ? w.before : now + w.before
  return { after: before + w.after, before }
}
