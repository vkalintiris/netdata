import { useQuery } from '@tanstack/react-query'
import { Check, Search } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Panel } from '@/components/Panel'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { totals, type Filters } from '@/lib/agent'
import { formatCount } from '@/lib/format'
import { KINDS, LABELS, OUTCOMES, usePageState, type FilterName, type Kind, type Outcome } from '@/lib/page-state'
import { useSeries } from '@/lib/queries'
import { K, SPAN_KEY_OF, spanStack } from '@/lib/traces'
import { cn } from '@/lib/utils'

type Source = 'metrics' | 'spans'

type Choice = { value: string; count: number; unmatchable?: string }

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
  choices,
  chosen,
  onToggle,
  searchable,
}: {
  label: string
  choices: Choice[] | undefined
  chosen: string[]
  onToggle: (value: string) => void
  searchable: boolean
}) {
  const [term, setTerm] = useState('')
  const typed = term.trim()
  const rows = useMemo(() => {
    const out = [...(choices ?? [])].filter((c) => c.count > 0)
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
          <li className="py-6 text-center text-sm text-muted-foreground">No calls</li>
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
  const { window, filters, outcomes, kinds, selected, toggleFilter, clearFilter } = usePageState()
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
  const spans = useQuery({
    queryKey: ['span-values', window, filters, outcomes, kinds, key],
    queryFn: ({ signal }) => spanStack(window, without(filters, key), outcomes, kinds, SPAN_KEY_OF[key], signal),
    enabled: source === 'spans',
    refetchInterval: window.after < 0 ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const choices = useMemo((): Choice[] | undefined => {
    if (source === 'metrics') {
      if (!metrics.data) return undefined
      // The Agent reports a missing label as "[unset]", which no filter can match.
      return Object.entries(totals(metrics.data)).map(([value, count]) => ({
        value,
        count,
        unmatchable: value === '[unset]' ? UNSET : undefined,
      }))
    }
    const t = spans.data
    if (!t) return undefined
    const out: Choice[] = Object.entries(t.values).map(([value, count]) => ({ value, count }))
    if (t.unset) out.push({ value: '[unset]', count: t.unset, unmatchable: UNSET })
    if (t.other) out.push({ value: '[other]', count: t.other, unmatchable: OTHER })
    return out
  }, [source, metrics.data, spans.data])

  return (
    <Panel
      title={label}
      subtitle={source === 'metrics' ? 'calls' : 'spans'}
      action={chosen.length > 0 && <ClearButton onClick={() => clearFilter(name)} />}
    >
      <ChoiceList label={label} choices={choices} chosen={chosen} onToggle={(v) => toggleFilter(name, v)} searchable />
    </Panel>
  )
}

// What a span is: a model call or one run of a tool. The Metrics page charts the two apart, so it has no such filter.
function KindPanel() {
  const { window, filters, outcomes, kinds, toggleKind, clearFilter } = usePageState()
  const spans = useQuery({
    queryKey: ['span-values', window, filters, outcomes, [], K.kind],
    queryFn: ({ signal }) => spanStack(window, filters, outcomes, [], K.kind, signal),
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
      subtitle="spans"
      action={kinds.length > 0 && <ClearButton onClick={() => clearFilter('kind')} />}
    >
      <ChoiceList label="Kind" choices={choices} chosen={kinds} onToggle={(v) => toggleKind(v as Kind)} searchable={false} />
    </Panel>
  )
}

// The outcome is no label of the charts' filters, so the Metrics page has no such filter.
function OutcomePanel() {
  const { window, filters, outcomes, kinds, toggleOutcome, clearFilter } = usePageState()
  const spans = useQuery({
    queryKey: ['span-values', window, filters, [], kinds, K.outcome],
    queryFn: ({ signal }) => spanStack(window, filters, [], kinds, K.outcome, signal),
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
      subtitle="spans"
      action={outcomes.length > 0 && <ClearButton onClick={() => clearFilter('outcome')} />}
    >
      <ChoiceList
        label="Outcome"
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

// The page filters as panels of values with their counts: calls from the GenAI charts on Metrics, stored spans on
// Spans and Traces, so a panel offers only values its page can show.
export function FilterPanels({ source }: { source: Source }) {
  return (
    <div className={cn('grid grid-cols-1 gap-3 md:grid-cols-3', source === 'spans' && 'xl:grid-cols-5')}>
      {PANELS.map((p) => (
        <FilterPanel key={p.name} {...p} source={source} />
      ))}
      {source === 'spans' && <KindPanel />}
      {source === 'spans' && <OutcomePanel />}
    </div>
  )
}
