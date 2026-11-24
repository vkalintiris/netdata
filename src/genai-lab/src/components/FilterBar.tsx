import { Check, ChevronDown, X } from 'lucide-react'
import { useQuery } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { totals, type Filters } from '@/lib/agent'
import { formatCount } from '@/lib/format'
import { LABELS, usePageState, type FilterName } from '@/lib/page-state'
import { useSeries } from '@/lib/queries'
import { SPAN_KEY_OF, spanStack } from '@/lib/traces'
import { cn } from '@/lib/utils'

const NAMES: { name: FilterName; label: string }[] = [
  { name: 'service', label: 'Service' },
  { name: 'model', label: 'Model' },
  { name: 'provider', label: 'Provider' },
]

function without(filters: Filters, key: string): Filters {
  const out = { ...filters }
  delete out[key]
  return out
}

function FilterPicker({ name, label, fromSpans }: { name: FilterName; label: string; fromSpans?: boolean }) {
  const { window, filters, outcomes, selected, toggleFilter } = usePageState()
  const key = LABELS[name]
  // The choices honour the other filters, not this one, so a selection can be widened.
  const { data } = useSeries({
    context: 'otel.derived_metrics.genai.calls',
    window,
    points: 1,
    timeGroup: 'sum',
    aggregation: 'sum',
    groupByLabels: [key],
    filters: without(filters, key),
  })
  const spanKey = fromSpans ? SPAN_KEY_OF[key] : undefined
  const spans = useQuery({
    queryKey: ['span-values', window, filters, outcomes, key],
    queryFn: ({ signal }) => spanStack(window, without(filters, key), outcomes, spanKey!, signal),
    enabled: spanKey != null,
    placeholderData: (prev) => prev,
  })
  const choices = useMemo(() => {
    const t = data ? { ...totals(data) } : {}
    // Span counts fill in values the metrics no longer hold; where both exist the metric count (calls) wins.
    for (const [v, n] of Object.entries(spans.data?.values ?? {})) if (!(v in t) || !t[v]) t[v] = n
    return Object.entries(t)
      .filter(([, v]) => v > 0)
      .sort((a, b) => b[1] - a[1])
  }, [data, spans.data])
  const chosen = selected(name)
  const [term, setTerm] = useState('')
  const typed = term.trim()

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className={cn(chosen.length && 'border-primary/60')}>
          <span className="text-muted-foreground">{label}</span>
          <span className="max-w-48 truncate">
            {chosen.length === 0 ? 'All' : chosen.length === 1 ? chosen[0] : `${chosen.length} selected`}
          </span>
          <ChevronDown className="opacity-60" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-80 p-0" align="start">
        <Command filter={(value, search) => (value.toLowerCase().includes(search.toLowerCase()) ? 1 : 0)}>
          <CommandInput placeholder={`Find a ${label.toLowerCase()}`} value={term} onValueChange={setTerm} />
          <CommandList>
            <CommandEmpty>No {label.toLowerCase()} with calls in this window</CommandEmpty>
            {/* The lists hold the busiest values only; any exact value can still be typed. */}
            {typed && !choices.some(([v]) => v === typed) && (
              <CommandGroup>
                <CommandItem value={typed} onSelect={() => toggleFilter(name, typed)}>
                  <Check className={cn('size-4', chosen.includes(typed) ? 'opacity-100' : 'opacity-0')} />
                  <span className="flex-1 truncate">
                    Filter by “{typed}”
                  </span>
                </CommandItem>
              </CommandGroup>
            )}
            <CommandGroup>
              {choices.map(([value, count]) => (
                <CommandItem key={value} value={value} onSelect={() => toggleFilter(name, value)}>
                  <Check className={cn('size-4', chosen.includes(value) ? 'opacity-100' : 'opacity-0')} />
                  <span className="flex-1 truncate">{value}</span>
                  <span className="font-mono text-xs text-muted-foreground tabular-nums">{formatCount(count)}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

export function FilterBar({ fromSpans }: { fromSpans?: boolean }) {
  const { filters, outcomes, clearFilters } = usePageState()
  const active = Object.keys(filters).length > 0 || (fromSpans === true && outcomes.length > 0)
  return (
    <div className="flex flex-wrap items-center gap-2">
      {NAMES.map((n) => (
        <FilterPicker key={n.name} {...n} fromSpans={fromSpans} />
      ))}
      {active && (
        <Button variant="ghost" size="sm" onClick={clearFilters}>
          <X />
          Clear filters
        </Button>
      )}
    </div>
  )
}
