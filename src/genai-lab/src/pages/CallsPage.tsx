import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { Info, X } from 'lucide-react'
import { useMemo } from 'react'
import { useSearchParams } from 'react-router'
import { CallDetail } from '@/components/CallDetail'
import { FilterBar } from '@/components/FilterBar'
import { Panel } from '@/components/Panel'
import { Ranking, type RankRow } from '@/components/Ranking'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, OUTCOMES, usePageState, type FilterName, type Outcome } from '@/lib/page-state'
import { Heatmap, type HeatCell, type HeatmapData } from '@/components/Heatmap'
import {
  cellParam,
  exploreCalls,
  field,
  K,
  parseCell,
  spanStack,
  stackTotals,
  type CallCell,
  type ExploreAnswer,
  type SpanRow,
  type StackTotals,
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

// A breakdown as ranked rows; spans without the attribute and values past the Function's stack limit show as rows
// no filter can match.
function stackRows(t: StackTotals | undefined): RankRow[] {
  if (!t) return []
  const rows: RankRow[] = Object.entries(t.values).map(([name, value]) => ({ key: name, name, value }))
  if (t.unset) rows.push({ key: '[unset]', name: '[unset]', value: t.unset })
  if (t.other) rows.push({ key: '[other]', name: '[other]', value: t.other })
  return rows
}

function outcomeRows(t: StackTotals | undefined): RankRow[] {
  if (!t) return []
  return (Object.keys(OUTCOMES) as Outcome[]).map((o) => ({
    key: o,
    name: o,
    value: OUTCOMES[o].reduce((sum, code) => sum + (t.values[code] ?? 0), 0),
  }))
}

function StatusBadge({ row }: { row: SpanRow }) {
  const status = field(row.fields, K.status)
  if (status === 'ERROR') {
    return (
      <Badge variant="destructive" title={field(row.fields, K.errorType)}>
        error
      </Badge>
    )
  }
  // Instrumentations differ on marking success (OK or left unset); both mean the call did not fail.
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

export function CallsPage() {
  const { window, filters, outcomes, selected: chosen, toggleFilter, toggleOutcome } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('call')
  const cell = parseCell(params.get('cell'))
  const live = window.after < 0 && !selected && !cell

  // The list follows the selected heatmap cell; the breakdowns and the heatmap keep showing the whole window.
  const query = useInfiniteQuery({
    queryKey: ['calls', window, filters, outcomes, cell],
    queryFn: ({ pageParam, signal }) => exploreCalls(window, filters, outcomes, pageParam, signal, cell),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const overview = useQuery({
    queryKey: ['calls-overview', window, filters, outcomes],
    queryFn: ({ signal }) => exploreCalls(window, filters, outcomes, undefined, signal, null, 1),
    enabled: cell != null,
    refetchInterval: window.after < 0 && !selected ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  // The list's own answer stacks by requested model; the other breakdowns each need a query of their own.
  const breakdown = (key: string) => ({
    queryKey: ['calls-by', key, window, filters, outcomes],
    queryFn: ({ signal }: { signal: AbortSignal }) => spanStack(window, filters, outcomes, key, signal),
    refetchInterval: window.after < 0 && !selected ? 10_000 : (false as const),
    placeholderData: (prev: StackTotals | undefined) => prev,
  })
  const byService = useQuery(breakdown(K.service))
  const byProvider = useQuery(breakdown(K.provider))
  const byStatus = useQuery(breakdown(K.status))

  const first = query.data?.pages[0]
  const whole = cell ? overview.data : first
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const byModel = useMemo(() => stackTotals(whole), [whole])
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
      next.delete('call')
      return next
    })
  const rank = (name: FilterName) => ({ selected: chosen(name), onSelect: (v: string) => toggleFilter(name, v) })

  const select = (row: SpanRow | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (row) next.set('call', `${row.trace_id}:${row.span_id}:${row.start_ns}`)
      else next.delete('call')
      return next
    })

  const range = absolute(window)
  const withDay = range.before - range.after > 12 * 3600 || new Date(range.after * 1000).getDate() !== new Date().getDate()

  return (
    <div className="flex flex-col gap-4">
      <FilterBar fromSpans />

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
        <Panel title="By service" subtitle="calls">
          <Ranking rows={stackRows(byService.data)} format={formatCount} empty="No calls" {...rank('service')} />
        </Panel>
        <Panel title="By model" subtitle="calls, by requested model">
          <Ranking rows={stackRows(byModel)} format={formatCount} empty="No calls" {...rank('model')} />
        </Panel>
        <Panel title="By provider" subtitle="calls">
          <Ranking rows={stackRows(byProvider.data)} format={formatCount} empty="No calls" {...rank('provider')} />
        </Panel>
        <Panel title="By outcome" subtitle="calls">
          <Ranking
            rows={outcomeRows(byStatus.data)}
            format={formatCount}
            empty="No calls"
            selected={outcomes}
            onSelect={(v) => toggleOutcome(v as Outcome)}
          />
        </Panel>
      </div>

      <Panel
        title="Call duration heatmap"
        subtitle={
          whole
            ? `${formatCount(whole.rows.matched)} spans per time and duration; click a cell to list its calls`
            : 'spans per time and duration; click a cell to list its calls'
        }
      >
        <Heatmap
          data={heat}
          formatBound={formatSeconds}
          selected={selectedCell}
          onCell={(c, d) => setCell(cellOf(c, d))}
          cellHint="Click to list these calls"
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
          Lists the spans that describe model calls in OpenTelemetry's GenAI conventions. A call reported by two
          instrumentations appears twice; calls reported in other conventions are counted on the Overview but not
          listed here. A span that names its provider only in the older gen_ai.system attribute shows as
          [unset] under By provider, and a provider filter does not match it.
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
            <p className="p-6 text-center text-sm text-muted-foreground">No model calls match in this window.</p>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className={withDay ? 'w-36' : 'w-24'}>Time</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead>Model</TableHead>
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
                        <TableCell className="max-w-56 truncate font-medium">
                          {field(row.fields, K.requestModel, K.responseModel) ?? '–'}
                        </TableCell>
                        <TableCell className="text-muted-foreground">
                          {field(row.fields, K.provider, K.system) ?? '–'}
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
        {selected && <CallDetail key={selected} id={selected} onClose={() => select(null)} />}
      </div>
    </div>
  )
}
