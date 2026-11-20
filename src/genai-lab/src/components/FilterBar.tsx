import { Check, ChevronDown, X } from 'lucide-react'
import { useMemo } from 'react'
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

function FilterPicker({ name, label }: { name: FilterName; label: string }) {
  const { window, filters, selected, toggleFilter } = usePageState()
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
  const choices = useMemo(() => {
    const t = data ? totals(data) : {}
    return Object.entries(t)
      .filter(([, v]) => v > 0)
      .sort((a, b) => b[1] - a[1])
  }, [data])
  const chosen = selected(name)

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
        <Command>
          <CommandInput placeholder={`Find a ${label.toLowerCase()}`} />
          <CommandList>
            <CommandEmpty>No {label.toLowerCase()} with calls in this window</CommandEmpty>
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

export function FilterBar() {
  const { filters, clearFilters } = usePageState()
  const active = Object.keys(filters).length > 0
  return (
    <div className="flex flex-wrap items-center gap-2">
      {NAMES.map((n) => (
        <FilterPicker key={n.name} {...n} />
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
