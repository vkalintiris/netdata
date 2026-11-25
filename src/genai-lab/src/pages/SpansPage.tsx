import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { Info, X } from 'lucide-react'
import { useMemo } from 'react'
import { useSearchParams } from 'react-router'
import { SpanDetail } from '@/components/SpanDetail'
import { FilterPanels } from '@/components/FilterPanels'
import { Panel } from '@/components/Panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, usePageState } from '@/lib/page-state'
import { Heatmap, type HeatCell, type HeatmapData } from '@/components/Heatmap'
import {
  cellParam,
  exploreSpans,
  field,
  K,
  parseCell,
  type CallCell,
  type ExploreAnswer,
  type SpanRow,
} from '@/lib/traces'
import { cn } from '@/lib/utils'

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
const dayTimeFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

function StatusBadge({ row }: { row: SpanRow }) {
  if (field(row.fields, K.outcome) === 'error') {
    return (
      <Badge variant="destructive" title={field(row.fields, K.errorType)}>
        error
      </Badge>
    )
  }
  return <span className="text-xs text-muted-foreground">ok</span>
}

// The Function's per-bucket duration counts as heatmap rows (bounds in ns, converted to seconds).
function spanHeatmap(a: ExploreAnswer | undefined): HeatmapData | undefined {
  const h = a?.histogram
  if (!a || !h?.duration_rows) return undefined
  const bounds = h.duration_rows
  const rows = bounds.map((upper, i) => ({
    lower: i === 0 ? 0 : (bounds[i - 1] ?? 0) / 1e9,
    upper: upper == null ? null : upper / 1e9,
  }))
  return {
    times: h.buckets.map((_, j) => a.window.bucket_start_s + j * a.window.bucket_width_s),
    step: a.window.bucket_width_s,
    after: a.window.after,
    before: a.window.before,
    rows,
    counts: rows.map((_, r) => h.buckets.map((b) => b.durations?.[r] ?? 0)),
  }
}

export function cellOf(c: HeatCell, d: HeatmapData): CallCell {
  const row = d.rows[c.row]
  const after = d.times[c.col]
  return { after, before: after + d.step, min: row.lower, max: row.upper }
}

const cellTime = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

export function SpansPage() {
  const { window, filters, outcomes, kinds } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('span')
  const cell = parseCell(params.get('cell'))
  const live = window.after < 0 && !selected && !cell

  // The list follows the selected heatmap cell; the heatmap keeps showing the whole window.
  const query = useInfiniteQuery({
    queryKey: ['spans', window, filters, outcomes, kinds, cell],
    queryFn: ({ pageParam, signal }) => exploreSpans(window, filters, outcomes, kinds, pageParam, signal, cell),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const overview = useQuery({
    queryKey: ['spans-overview', window, filters, outcomes, kinds],
    queryFn: ({ signal }) => exploreSpans(window, filters, outcomes, kinds, undefined, signal, null, 1),
    enabled: cell != null,
    refetchInterval: window.after < 0 && !selected ? 10_000 : false,
    placeholderData: (prev) => prev,
  })

  const first = query.data?.pages[0]
  const whole = cell ? overview.data : first
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const heat = useMemo(() => spanHeatmap(whole), [whole])
  const selectedCell = useMemo(() => {
    if (!cell || !heat) return null
    const col = heat.times.findIndex((t) => t === cell.after)
    const row = heat.rows.findIndex((r) => r.lower === cell.min)
    return col >= 0 && row >= 0 ? { col, row } : null
  }, [cell, heat])

  const setCell = (c: CallCell | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (c) next.set('cell', cellParam(c))
      else next.delete('cell')
      next.delete('span')
      return next
    })

  const select = (row: SpanRow | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (row) next.set('span', `${row.trace_id}:${row.span_id}:${row.start_ns}`)
      else next.delete('span')
      return next
    })

  const range = absolute(window)
  const withDay = range.before - range.after > 12 * 3600 || new Date(range.after * 1000).getDate() !== new Date().getDate()

  return (
    <div className="flex flex-col gap-4">
      <FilterPanels source="spans" />

      <Panel
        title="Span duration heatmap"
        subtitle={
          whole
            ? `${formatCount(whole.rows.matched)} spans per time and duration; click a cell to list its spans`
            : 'spans per time and duration; click a cell to list its spans'
        }
      >
        <Heatmap
          data={heat}
          formatBound={formatSeconds}
          selected={selectedCell}
          onCell={(c, d) => setCell(cellOf(c, d))}
          cellHint="Click to list these spans"
        />
      </Panel>

      {cell && (
        <div className="flex items-center gap-2 text-sm">
          <span className="rounded-md border border-primary/60 px-2 py-1">
            Listing {cellTime.format(cell.after * 1000)}–{cellTime.format(cell.before * 1000)}, duration{' '}
            {formatSeconds(cell.min)}–{cell.max == null ? '∞' : formatSeconds(cell.max)}
          </span>
          <Button variant="ghost" size="sm" onClick={() => setCell(null)}>
            <X />
            Whole window
          </Button>
        </div>
      )}

      <p className="flex items-start gap-2 text-xs text-muted-foreground">
        <Info className="mt-px size-3.5 shrink-0" />
        <span>
          Lists the spans the Metrics tab counts: model calls and tool calls, in any conventions. A model call
          reported by two instrumentations is listed once, by the span it is counted by. Other spans of a trace
          (agent steps, framework and application spans) show in the trace of a span, not here.
        </span>
      </p>

      <div className={cn('grid gap-4', selected && 'xl:grid-cols-[minmax(0,1fr)_560px]')}>
        <div className="min-w-0 rounded-lg border bg-card">
          {query.error ? (
            <p className="p-6 text-sm text-destructive">{String((query.error as Error).message)}</p>
          ) : !query.data ? (
            <div className="flex flex-col gap-2 p-4">
              {Array.from({ length: 8 }, (_, i) => (
                <Skeleton key={i} className="h-6 w-full" />
              ))}
            </div>
          ) : rows.length === 0 ? (
            <p className="p-6 text-center text-sm text-muted-foreground">No spans match in this window.</p>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className={withDay ? 'w-36' : 'w-24'}>Time</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead>Kind</TableHead>
                    <TableHead>Model or tool</TableHead>
                    <TableHead>Provider</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead className="text-right">Duration</TableHead>
                    <TableHead className="text-right">Tokens in</TableHead>
                    <TableHead className="text-right">Tokens out</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => {
                    const id = `${row.trace_id}:${row.span_id}:${row.start_ns}`
                    const active = selected === id
                    const tool = field(row.fields, K.kind) === 'tool'
                    const tokensIn = field(row.fields, K.inputTokens)
                    const tokensOut = field(row.fields, K.outputTokens)
                    return (
                      <TableRow
                        key={id}
                        onClick={() => select(active ? null : row)}
                        data-state={active ? 'selected' : undefined}
                        className="cursor-pointer"
                      >
                        <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                          {(withDay ? dayTimeFmt : timeFmt).format(row.start_ns / 1e6)}
                        </TableCell>
                        <TableCell className="max-w-56 truncate">{field(row.fields, K.service)}</TableCell>
                        <TableCell className="text-xs whitespace-nowrap text-muted-foreground">
                          {tool ? 'tool call' : 'model call'}
                        </TableCell>
                        <TableCell className="max-w-56 truncate font-medium">
                          {(tool ? field(row.fields, K.tool, K.name) : field(row.fields, K.requestModel)) ?? '–'}
                        </TableCell>
                        <TableCell className="text-muted-foreground">
                          {field(row.fields, K.provider) ?? '–'}
                        </TableCell>
                        <TableCell>
                          <StatusBadge row={row} />
                        </TableCell>
                        <TableCell className="text-right font-mono text-xs whitespace-nowrap tabular-nums">
                          {formatSeconds(row.duration_ns / 1e9)}
                        </TableCell>
                        <TableCell className="text-right font-mono text-xs tabular-nums">
                          {tokensIn ? formatCount(Number(tokensIn)) : '–'}
                        </TableCell>
                        <TableCell className="text-right font-mono text-xs tabular-nums">
                          {tokensOut ? formatCount(Number(tokensOut)) : '–'}
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
              <div className="flex items-center justify-between border-t px-4 py-2 text-xs text-muted-foreground">
                <span>
                  {formatCount(rows.length)} of {formatCount(first?.rows.matched)} shown, newest first
                </span>
                {query.hasNextPage && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => query.fetchNextPage()}
                    disabled={query.isFetchingNextPage}
                  >
                    {query.isFetchingNextPage ? 'Loading…' : 'Load more'}
                  </Button>
                )}
              </div>
            </>
          )}
        </div>
        {selected && <SpanDetail key={selected} id={selected} onClose={() => select(null)} />}
      </div>
    </div>
  )
}
