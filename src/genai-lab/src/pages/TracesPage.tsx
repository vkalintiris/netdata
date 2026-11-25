import { useInfiniteQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Info } from 'lucide-react'
import { Fragment, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router'
import { SpanDetail } from '@/components/SpanDetail'
import { FilterPanels } from '@/components/FilterPanels'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, usePageState } from '@/lib/page-state'
import { groupTraces, isTool, type TraceRow } from '@/lib/trace-list'
import { exploreSpans, field, K, type SpanRow } from '@/lib/traces'
import { cn } from '@/lib/utils'

// The most rows the Function returns per request.
const SPANS_PAGE = 200

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
const dayTimeFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

const num = 'text-right font-mono text-xs whitespace-nowrap tabular-nums'

function Tokens({ value }: { value: number | string | null | undefined }) {
  return <>{value == null || value === '' ? '–' : formatCount(Number(value))}</>
}

function Names({ values }: { values: string[] }) {
  if (values.length === 0) return <>–</>
  return <span title={values.join(', ')}>{values.join(', ')}</span>
}

function spanId(row: SpanRow): string {
  return `${row.trace_id}:${row.span_id}:${row.start_ns}`
}

function Failed({ count }: { count: number }) {
  if (count === 0) return null
  return (
    <Badge variant="destructive" className="ml-2">
      {formatCount(count)} failed
    </Badge>
  )
}

export function TracesPage() {
  const { window, filters, outcomes, kinds } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('span')
  const [open, setOpen] = useState<Set<string>>(new Set())
  const live = window.after < 0 && !selected && open.size === 0

  const query = useInfiniteQuery({
    queryKey: ['traces', window, filters, outcomes, kinds],
    queryFn: ({ pageParam, signal }) =>
      exploreSpans(window, filters, outcomes, kinds, pageParam, signal, null, SPANS_PAGE),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })

  const spans = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const traces = useMemo(() => groupTraces(spans), [spans])
  const matched = query.data?.pages[0]?.rows.matched

  const toggle = (trace: TraceRow) =>
    setOpen((prev) => {
      const next = new Set(prev)
      if (!next.delete(trace.traceId)) next.add(trace.traceId)
      return next
    })

  const select = (row: SpanRow | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (row) next.set('span', spanId(row))
      else next.delete('span')
      return next
    })

  const range = absolute(window)
  const withDay = range.before - range.after > 12 * 3600 || new Date(range.after * 1000).getDate() !== new Date().getDate()
  const fmt = withDay ? dayTimeFmt : timeFmt

  return (
    <div className="flex flex-col gap-4">
      <FilterPanels source="spans" />

      <p className="flex items-start gap-2 text-xs text-muted-foreground">
        <Info className="mt-px size-3.5 shrink-0" />
        <span>
          One row per trace: the model calls and tool calls the Spans tab lists, added up by the trace they
          belong to. The filters choose spans, so a filtered trace adds up only its matching spans. Wall time
          runs from the first of them to start to the last to end. An application that opens no span of its own
          around its calls sends one trace per call.
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
          ) : traces.length === 0 ? (
            <p className="p-6 text-center text-sm text-muted-foreground">No spans match in this window.</p>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className={withDay ? 'w-44' : 'w-32'}>Started</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead className="text-right">Model calls</TableHead>
                    <TableHead className="text-right">Tool calls</TableHead>
                    <TableHead>Models</TableHead>
                    <TableHead>Providers</TableHead>
                    <TableHead className="text-right">Wall time</TableHead>
                    <TableHead className="text-right">In calls</TableHead>
                    <TableHead className="text-right">Tokens in</TableHead>
                    <TableHead className="text-right">Tokens out</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {traces.map((trace) => {
                    const expanded = open.has(trace.traceId)
                    const Chevron = expanded ? ChevronDown : ChevronRight
                    return (
                      <Fragment key={trace.traceId}>
                        <TableRow onClick={() => toggle(trace)} className="cursor-pointer" aria-expanded={expanded}>
                          <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                            <Chevron className="mr-1 inline size-3.5" />
                            {fmt.format(trace.startNs / 1e6)}
                          </TableCell>
                          <TableCell className="max-w-56 truncate">{trace.service ?? '–'}</TableCell>
                          <TableCell className={num}>
                            {formatCount(trace.calls)}
                            <Failed count={trace.failedCalls} />
                          </TableCell>
                          <TableCell className={num}>
                            {trace.tools ? formatCount(trace.tools) : '–'}
                            <Failed count={trace.failedTools} />
                          </TableCell>
                          <TableCell className="max-w-56 truncate font-medium">
                            <Names values={trace.models} />
                          </TableCell>
                          <TableCell className="max-w-40 truncate text-muted-foreground">
                            <Names values={trace.providers} />
                          </TableCell>
                          <TableCell className={num}>{formatSeconds((trace.endNs - trace.startNs) / 1e9)}</TableCell>
                          <TableCell className={num}>{formatSeconds(trace.inCallsNs / 1e9)}</TableCell>
                          <TableCell className={num}>
                            <Tokens value={trace.tokensIn} />
                          </TableCell>
                          <TableCell className={num}>
                            <Tokens value={trace.tokensOut} />
                          </TableCell>
                        </TableRow>
                        {expanded &&
                          trace.spans.map((row) => {
                            const id = spanId(row)
                            const tool = isTool(row)
                            const active = selected === id
                            const failed = field(row.fields, K.outcome) === 'error'
                            return (
                              <TableRow
                                key={id}
                                onClick={() => select(active ? null : row)}
                                data-state={active ? 'selected' : undefined}
                                className="cursor-pointer bg-muted/30"
                              >
                                <TableCell className="pl-8 font-mono text-xs whitespace-nowrap text-muted-foreground">
                                  {fmt.format(row.start_ns / 1e6)}
                                </TableCell>
                                <TableCell className="text-xs text-muted-foreground">
                                  {tool ? 'tool call' : 'model call'}
                                </TableCell>
                                <TableCell className="text-right" colSpan={2}>
                                  {failed && (
                                    <Badge variant="destructive" title={field(row.fields, K.errorType)}>
                                      error
                                    </Badge>
                                  )}
                                </TableCell>
                                <TableCell className="max-w-56 truncate">
                                  {(tool ? field(row.fields, K.tool, K.name) : field(row.fields, K.requestModel)) ?? '–'}
                                </TableCell>
                                <TableCell className="max-w-40 truncate text-muted-foreground">
                                  {tool ? '' : (field(row.fields, K.provider) ?? '–')}
                                </TableCell>
                                <TableCell className={num}>{formatSeconds(row.duration_ns / 1e9)}</TableCell>
                                <TableCell />
                                <TableCell className={num}>{!tool && <Tokens value={field(row.fields, K.inputTokens)} />}</TableCell>
                                <TableCell className={num}>{!tool && <Tokens value={field(row.fields, K.outputTokens)} />}</TableCell>
                              </TableRow>
                            )
                          })}
                      </Fragment>
                    )
                  })}
                </TableBody>
              </Table>
              <div className="flex items-center justify-between gap-4 border-t px-4 py-2 text-xs text-muted-foreground">
                <span>
                  {formatCount(traces.length)} traces from the latest {formatCount(spans.length)} of{' '}
                  {formatCount(matched)} spans
                  {query.hasNextPage && '; the oldest traces listed may miss their earlier spans'}
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
