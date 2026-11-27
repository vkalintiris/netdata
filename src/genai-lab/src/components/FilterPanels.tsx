import { useQuery } from '@tanstack/react-query'
import { Check, ChevronDown, Search, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import type React from 'react'
import { Panel } from '@/components/Panel'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { totals, type Filters } from '@/lib/agent'
import { formatCount } from '@/lib/format'
import {
  ALL_KINDS,
  KINDS,
  LABELS,
  OUTCOMES,
  unitOf,
  usePageState,
  type FilterName,
  type Kind,
  type Outcome,
} from '@/lib/page-state'
import { useSeries } from '@/lib/queries'
import { K, SPAN_KEY_OF, spanStack } from '@/lib/traces'
import { cn } from '@/lib/utils'

type Source = 'metrics' | 'spans'

// `listed` keeps a value with no count in the list.
type Choice = { value: string; count: number; unmatchable?: string; listed?: boolean }

const PANELS: { name: FilterName; label: string }[] = [
  { name: 'service', label: 'Service' },
  { name: 'model', label: 'Model' },
  { name: 'provider', label: 'Provider' },
]

const UNSET = 'The attribute is missing on these calls or spans'
const OTHER = 'Values past the query limit, added up; type a value to filter by it'

function without(filters: Filters, key: string): Filters {
  const out = { ...filters }
  delete out[key]
  return out
}

function ChoiceList({
  label,
  unit,
  choices,
  chosen,
  onToggle,
  searchable,
}: {
  label: string
  unit: string
  choices: Choice[] | undefined
  chosen: string[]
  onToggle: (value: string) => void
  searchable: boolean
}) {
  const [term, setTerm] = useState('')
  const typed = term.trim()
  const rows = useMemo(() => {
    const out = [...(choices ?? [])].filter((c) => c.count > 0 || c.listed)
    // A selection with no calls in this window stays listed, so it can be removed here.
    for (const v of chosen) if (!out.some((c) => c.value === v)) out.push({ value: v, count: 0 })
    out.sort((a, b) => b.count - a.count)
    const needle = typed.toLowerCase()
    return needle ? out.filter((c) => c.value.toLowerCase().includes(needle)) : out
  }, [choices, chosen, typed])
  const max = Math.max(1, ...rows.map((r) => r.count))

  if (!choices) return <Skeleton className="h-56 w-full" />
  return (
    <div className="flex flex-col gap-1.5">
      {searchable && (
        <label className="flex items-center gap-1.5 rounded-md border px-2 py-1 text-sm">
          <Search className="size-3.5 shrink-0 text-muted-foreground" />
          <input
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
            placeholder={`Find a ${label.toLowerCase()}`}
            value={term}
            onChange={(e) => setTerm(e.target.value)}
          />
        </label>
      )}
      <ul className="flex h-56 flex-col gap-0.5 overflow-y-auto">
        {/* The lists can stop at a query limit; any exact value can still be typed. */}
        {typed && !rows.some((r) => r.value === typed) && (
          <li>
            <button
              type="button"
              onClick={() => onToggle(typed)}
              className="w-full truncate rounded px-2 py-1 text-left text-sm hover:bg-muted"
            >
              Filter by “{typed}”
            </button>
          </li>
        )}
        {rows.length === 0 && !typed && (
          <li className="py-6 text-center text-sm text-muted-foreground">No {unit}</li>
        )}
        {rows.map((r) => {
          const active = chosen.includes(r.value)
          return (
            <li key={r.value}>
              <button
                type="button"
                disabled={r.unmatchable != null}
                onClick={() => onToggle(r.value)}
                title={r.unmatchable ?? (active ? 'Remove from the filter' : 'Add to the filter')}
                className={cn(
                  'relative flex w-full items-center gap-1.5 overflow-hidden rounded px-2 py-1 text-left text-sm hover:bg-muted disabled:cursor-default disabled:text-muted-foreground disabled:hover:bg-transparent',
                  active && 'ring-1 ring-primary',
                )}
              >
                <span className="absolute inset-y-0 left-0 bg-primary/12" style={{ width: `${(r.count / max) * 100}%` }} />
                <Check className={cn('relative size-3.5 shrink-0 text-primary', !active && 'invisible')} />
                <span className="relative min-w-0 flex-1 truncate">{r.value}</span>
                <span className="relative font-mono text-xs tabular-nums">{formatCount(r.count)}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

function FilterPanel({ name, label, source }: { name: FilterName; label: string; source: Source }) {
  const { window, filters, spanQuery, selected, toggleFilter, clearFilter } = usePageState()
  const key = LABELS[name]
  const chosen = selected(name)
  // Each panel honours the other filters, not its own, so a selection can be widened.
  const metrics = useSeries(
    source === 'metrics'
      ? {
          context: 'otel.derived_metrics.genai.calls',
          window,
          points: 1,
          timeGroup: 'sum',
          aggregation: 'sum',
          groupByLabels: [key],
          filters: without(filters, key),
        }
      : null,
  )
  // A service that reports tool calls and no model call is on the Metrics page too, in its Tools section.
  const toolServices = useSeries(
    source === 'metrics' && name === 'service'
      ? {
          context: 'otel.derived_metrics.genai.tool_calls',
          window,
          points: 1,
          timeGroup: 'sum',
          aggregation: 'sum',
          groupByLabels: [key],
        }
      : null,
  )
  const query = useMemo(() => ({ ...spanQuery, filters: without(filters, key) }), [spanQuery, filters, key])
  const spans = useQuery({
    queryKey: ['span-values', window, query, key],
    queryFn: ({ signal }) => spanStack(window, query, SPAN_KEY_OF[key], signal),
    enabled: source === 'spans',
    refetchInterval: window.after < 0 ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const choices = useMemo((): Choice[] | undefined => {
    if (source === 'metrics') {
      if (!metrics.data) return undefined
      // The Agent reports a missing label as "[unset]", which no filter can match.
      const out: Choice[] = Object.entries(totals(metrics.data)).map(([value, count]) => ({
        value,
        count,
        unmatchable: value === '[unset]' ? UNSET : undefined,
      }))
      for (const [value, count] of Object.entries(toolServices.data ? totals(toolServices.data) : {})) {
        if (count > 0 && value !== '[unset]' && !out.some((c) => c.value === value)) {
          out.push({ value, count: 0, listed: true })
        }
      }
      return out
    }
    const t = spans.data
    if (!t) return undefined
    const out: Choice[] = Object.entries(t.values).map(([value, count]) => ({ value, count }))
    if (t.unset) out.push({ value: '[unset]', count: t.unset, unmatchable: UNSET })
    if (t.other) out.push({ value: '[other]', count: t.other, unmatchable: OTHER })
    return out
  }, [source, metrics.data, toolServices.data, spans.data])

  // The GenAI charts count model calls.
  const unit = source === 'metrics' ? 'model calls' : unitOf(spanQuery.kinds)
  return (
    <Panel
      title={label}
      subtitle={unit}
      action={chosen.length > 0 && <ClearButton onClick={() => clearFilter(name)} />}
    >
      <ChoiceList
        label={label}
        unit={unit}
        choices={choices}
        chosen={chosen}
        onToggle={(v) => toggleFilter(name, v)}
        searchable
      />
    </Panel>
  )
}

// What a span is: a model call or one run of a tool. The Metrics page charts the two apart, so it has no such filter.
function KindPanel() {
  const { window, spanQuery, kinds, toggleKind, clearFilter } = usePageState()
  // Each kind's count is what choosing it would show, whatever the tab's default.
  const query = useMemo(() => ({ ...spanQuery, kinds: ALL_KINDS }), [spanQuery])
  const spans = useQuery({
    queryKey: ['span-values', window, query, K.kind],
    queryFn: ({ signal }) => spanStack(window, query, K.kind, signal),
    refetchInterval: window.after < 0 ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const choices = useMemo((): Choice[] | undefined => {
    const t = spans.data
    if (!t) return undefined
    return (Object.keys(KINDS) as Kind[]).map((k) => ({ value: k, count: t.values[KINDS[k]] ?? 0 }))
  }, [spans.data])

  return (
    <Panel
      title="Kind"
      subtitle={unitOf(ALL_KINDS)}
      action={kinds.length > 0 && <ClearButton onClick={() => clearFilter('kind')} />}
    >
      <ChoiceList
        label="Kind"
        unit={unitOf(ALL_KINDS)}
        choices={choices}
        chosen={kinds}
        onToggle={(v) => toggleKind(v as Kind)}
        searchable={false}
      />
    </Panel>
  )
}

// The outcome is no label of the charts' filters, so the Metrics page has no such filter.
function OutcomePanel() {
  const { window, spanQuery, outcomes, toggleOutcome, clearFilter } = usePageState()
  const query = useMemo(() => ({ ...spanQuery, outcomes: [] }), [spanQuery])
  const spans = useQuery({
    queryKey: ['span-values', window, query, K.outcome],
    queryFn: ({ signal }) => spanStack(window, query, K.outcome, signal),
    refetchInterval: window.after < 0 ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const choices = useMemo((): Choice[] | undefined => {
    const t = spans.data
    if (!t) return undefined
    return (Object.keys(OUTCOMES) as Outcome[]).map((o) => ({
      value: o,
      count: t.values[OUTCOMES[o]] ?? 0,
    }))
  }, [spans.data])

  return (
    <Panel
      title="Outcome"
      subtitle={unitOf(spanQuery.kinds)}
      action={outcomes.length > 0 && <ClearButton onClick={() => clearFilter('outcome')} />}
    >
      <ChoiceList
        label="Outcome"
        unit={unitOf(spanQuery.kinds)}
        choices={choices}
        chosen={outcomes}
        onToggle={(v) => toggleOutcome(v as Outcome)}
        searchable={false}
      />
    </Panel>
  )
}

function ClearButton({ onClick }: { onClick: () => void }) {
  return (
    <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={onClick}>
      Clear
    </Button>
  )
}

// The header every OTel GenAI tab opens with: Service, Model and Provider as panels of values with their counts.
// Counts come from the tab's own source, calls from the GenAI charts on Metrics and stored spans on Breakdown and
// Traces, so a panel offers only values its tab can show.
export function FilterPanels({ source }: { source: Source }) {
  return (
    <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
      {PANELS.map((p) => (
        <FilterPanel key={p.name} {...p} source={source} />
      ))}
    </div>
  )
}

function BarButton({ label, count, children }: { label: string; count: number; children: React.ReactNode }) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className={cn('gap-1.5', count > 0 && 'border-primary/60')}>
          {label}
          {count > 0 && <span className="rounded bg-primary/15 px-1 font-mono text-xs tabular-nums">{count}</span>}
          <ChevronDown className="size-3.5 text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      {/* The panel asks for its values only while it is open. */}
      <PopoverContent align="start" className="w-80 p-0 [&>section]:border-0">
        {children}
      </PopoverContent>
    </Popover>
  )
}

// The span-only filters, as buttons among a list tab's own controls: Kind, Outcome and conditions set elsewhere.
export function SpanFilterBar() {
  const { kinds, outcomes, where, removeWhere } = usePageState()
  return (
    <div className="flex flex-wrap items-center gap-2">
      <BarButton label="Kind" count={kinds.length}>
        <KindPanel />
      </BarButton>
      <BarButton label="Outcome" count={outcomes.length}>
        <OutcomePanel />
      </BarButton>
      {Object.entries(where).flatMap(([key, values]) =>
        values.map((value) => (
          <span
            key={`${key}=${value}`}
            className="flex items-center gap-1 rounded-md border border-primary/60 py-1 pr-1 pl-2 text-xs"
          >
            <span className="text-muted-foreground">{key.replace(/^(resource\.)?attributes\./, '')}</span>
            <span className="font-medium">{value}</span>
            <button
              type="button"
              aria-label="Remove this filter"
              className="rounded p-0.5 hover:bg-muted"
              onClick={() => removeWhere(key, value)}
            >
              <X className="size-3" />
            </button>
          </span>
        )),
      )}
    </div>
  )
}
