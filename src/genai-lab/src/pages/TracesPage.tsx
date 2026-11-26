import { X } from 'lucide-react'
import { useSearchParams } from 'react-router'
import { FilterPanels, SpanFilterBar } from '@/components/FilterPanels'
import { SpanDetail } from '@/components/SpanDetail'
import { SpanList, spanKey } from '@/components/SpanList'
import { TraceList, traceLink } from '@/components/TraceList'
import { Button } from '@/components/ui/button'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { formatSeconds } from '@/lib/format'
import { usePageState } from '@/lib/page-state'
import { parseCell, type SpanRow } from '@/lib/traces'
import { cn } from '@/lib/utils'

const cellTime = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

// The place to find one trace or one call: a list under the page's filters, by trace or by span.
export function TracesPage() {
  const { window } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('span')
  // A time and duration range handed over by a heatmap; it narrows the span rows.
  const cell = parseCell(params.get('cell'))
  const bySpan = params.get('rows') === 'spans' || cell != null
  const live = window.after < 0 && !selected && !cell

  const update = (fn: (p: URLSearchParams) => void) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      fn(next)
      return next
    })

  const setRows = (rows: string) =>
    update((p) => {
      if (rows === 'spans') p.set('rows', 'spans')
      else p.delete('rows')
      p.delete('cell')
      p.delete('span')
    })

  const select = (row: SpanRow | null) =>
    update((p) => {
      if (row) p.set('span', spanKey(row))
      else p.delete('span')
    })

  return (
    <div className="flex flex-col gap-3">
      <FilterPanels source="spans" />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={bySpan ? 'spans' : 'traces'}
          onValueChange={(v) => v && setRows(v)}
        >
          <ToggleGroupItem value="traces" title="One row per trace: its model calls and tool calls added up">
            Traces
          </ToggleGroupItem>
          <ToggleGroupItem value="spans" title="One row per model call or tool call">
            Calls
          </ToggleGroupItem>
        </ToggleGroup>
        <SpanFilterBar />
        {cell && (
          <span className="flex items-center gap-1 rounded-md border border-primary/60 py-1 pr-1 pl-2 text-xs">
            {cellTime.format(cell.after * 1000)}–{cellTime.format(cell.before * 1000)}, lasting{' '}
            {formatSeconds(cell.min)}–{cell.max == null ? '∞' : formatSeconds(cell.max)}
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-5"
              aria-label="Whole window"
              onClick={() => update((p) => p.delete('cell'))}
            >
              <X className="size-3" />
            </Button>
          </span>
        )}
      </div>

      {bySpan ? (
        <div className={cn('grid gap-4', selected && 'xl:grid-cols-[minmax(0,1fr)_560px]')}>
          <SpanList cell={cell} selected={selected} onSelect={select} live={live} />
          {selected && (
            <SpanDetail
              key={selected}
              id={selected}
              onClose={() => select(null)}
              traceHref={traceLink(selected.split(':')[0], Number(selected.split(':')[2]), params)}
            />
          )}
        </div>
      ) : (
        <TraceList live={live} />
      )}
    </div>
  )
}
