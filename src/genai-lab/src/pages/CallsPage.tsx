import { useInfiniteQuery } from '@tanstack/react-query'
import { Info } from 'lucide-react'
import { useCallback, useMemo } from 'react'
import { useSearchParams } from 'react-router'
import { CallDetail } from '@/components/CallDetail'
import { FilterBar } from '@/components/FilterBar'
import { Panel } from '@/components/Panel'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import type { Series } from '@/lib/agent'
import { formatCount, formatInterval, formatSeconds } from '@/lib/format'
import { absolute, usePageState } from '@/lib/page-state'
import { exploreCalls, field, K, unappliedFilters, type ExploreAnswer, type SpanRow } from '@/lib/traces'
import { cn } from '@/lib/utils'

const PALETTE = ['--chart-1', '--chart-2', '--chart-3', '--chart-4', '--chart-5', '--chart-6', '--chart-7', '--chart-8']
const STACK_LIMIT = 7

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

// The Function's histogram as a chart series: the busiest models keep their own layer, the rest add up to "other".
function histogramSeries(a: ExploreAnswer | undefined): { series: Series; styles: SeriesStyle[] } | undefined {
  const h = a?.histogram
  if (!a || !h) return undefined
  const totals = h.dimensions.map((_, i) => h.buckets.reduce((s, b) => s + (b.counts[i] ?? 0), 0))
  const order = totals
    .map((t, i) => ({ t, i }))
    .filter((x) => x.t > 0)
    .sort((x, y) => y.t - x.t)
  const kept = order.slice(0, STACK_LIMIT)
  const keptIdx = new Set(kept.map((k) => k.i))
  const labels = kept.map((k) => h.dimensions[k.i] || '(unset)')
  const times = h.buckets.map((_, j) => a.window.bucket_start_s + j * a.window.bucket_width_s)
  const values = kept.map((k) => h.buckets.map((b) => b.counts[k.i] ?? 0))
  const rest = h.buckets.map(
    (b) => b.other + b.unset + b.counts.reduce((s, c, i) => (keptIdx.has(i) ? s : s + c), 0),
  )
  if (rest.some((v) => v > 0)) {
    labels.push('other')
    values.push(rest)
  }
  const styles = labels.map((label, i) => ({
    label,
    color: label === 'other' ? '--muted-foreground' : PALETTE[i % PALETTE.length],
  }))
  return {
    series: {
      labels,
      times,
      values,
      units: 'calls',
      pointSeconds: a.window.bucket_width_s,
      after: a.window.after,
      before: a.window.before,
    },
    styles,
  }
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
  return <span className="text-xs text-muted-foreground">{status === 'OK' ? 'ok' : 'unset'}</span>
}

const fmtCount = (v: number | null) => formatCount(v)

export function CallsPage() {
  const { window, filters, setWindow } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('call')

  const query = useInfiniteQuery({
    queryKey: ['calls', window, filters],
    queryFn: ({ pageParam, signal }) => exploreCalls(window, filters, pageParam, signal),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: window.after < 0 && !selected ? 10_000 : false,
    placeholderData: (prev) => prev,
  })

  const first = query.data?.pages[0]
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const chart = useMemo(() => histogramSeries(first), [first])
  const unapplied = unappliedFilters(filters)

  const select = (row: SpanRow | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (row) next.set('call', `${row.trace_id}:${row.span_id}:${row.start_ns}`)
      else next.delete('call')
      return next
    })

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
  const onReset = useCallback(() => setWindow({ after: -3600, before: 0 }), [setWindow])

  return (
    <div className="flex flex-col gap-4">
      <FilterBar />

      <Panel
        title="Model calls"
        subtitle={
          first
            ? `${formatCount(first.rows.matched)} spans · per ${formatInterval(first.window.bucket_width_s)}, by requested model`
            : undefined
        }
      >
        {chart ? (
          <TimeChart
            data={chart.series}
            styles={chart.styles}
            stacked
            format={fmtCount}
            height={150}
            onZoom={onZoom}
            onPan={onPan}
            onReset={onReset}
          />
        ) : (
          <Skeleton className="h-[170px] w-full" />
        )}
      </Panel>

      <p className="flex items-start gap-2 text-xs text-muted-foreground">
        <Info className="mt-px size-3.5 shrink-0" />
        <span>
          Lists the spans that describe model calls in OpenTelemetry's GenAI conventions. A call reported by two
          instrumentations appears twice; calls reported only as log events or in other conventions are counted on the
          Overview but not listed here.
          {unapplied.length > 0 && ' The Provider filter does not apply to this list.'}
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
                    <TableHead className="w-24">Time</TableHead>
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
                        <TableCell className="font-mono text-xs text-muted-foreground">
                          {timeFmt.format(row.start_ns / 1e6)}
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
                        <TableCell className="text-right font-mono text-xs tabular-nums">
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
