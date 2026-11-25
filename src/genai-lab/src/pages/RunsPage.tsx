import { useInfiniteQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Info } from 'lucide-react'
import { Fragment, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router'
import { CallDetail } from '@/components/CallDetail'
import { FilterPanels } from '@/components/FilterPanels'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, usePageState } from '@/lib/page-state'
import { groupRuns, type Run } from '@/lib/runs'
import { exploreCalls, field, K, type SpanRow } from '@/lib/traces'
import { cn } from '@/lib/utils'

// The most rows the Function returns per request.
const CALLS_PAGE = 200

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

function callId(row: SpanRow): string {
  return `${row.trace_id}:${row.span_id}:${row.start_ns}`
}

export function RunsPage() {
  const { window, filters, outcomes } = usePageState()
  const [params, setParams] = useSearchParams()
  const selected = params.get('call')
  const [open, setOpen] = useState<Set<string>>(new Set())
  const live = window.after < 0 && !selected && open.size === 0

  const query = useInfiniteQuery({
    queryKey: ['runs', window, filters, outcomes],
    queryFn: ({ pageParam, signal }) =>
      exploreCalls(window, filters, outcomes, pageParam, signal, null, CALLS_PAGE),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })

  const calls = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const runs = useMemo(() => groupRuns(calls), [calls])
  const matched = query.data?.pages[0]?.rows.matched

  const toggle = (run: Run) =>
    setOpen((prev) => {
      const next = new Set(prev)
      if (!next.delete(run.traceId)) next.add(run.traceId)
      return next
    })

  const select = (row: SpanRow | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (row) next.set('call', callId(row))
      else next.delete('call')
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
          One row per trace: the model calls the Calls tab lists, added up by the trace they belong to. The
          filters choose calls, so a filtered run adds up only its matching calls. Wall time runs from the first
          call's start to the last call's end. An application that opens no span of its own around its calls
          sends one trace per call.
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
          ) : runs.length === 0 ? (
            <p className="p-6 text-center text-sm text-muted-foreground">No model calls match in this window.</p>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className={withDay ? 'w-44' : 'w-32'}>Started</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead className="text-right">Calls</TableHead>
                    <TableHead>Models</TableHead>
                    <TableHead>Providers</TableHead>
                    <TableHead className="text-right">Wall time</TableHead>
                    <TableHead className="text-right">In calls</TableHead>
                    <TableHead className="text-right">Tokens in</TableHead>
                    <TableHead className="text-right">Tokens out</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {runs.map((run) => {
                    const expanded = open.has(run.traceId)
                    const Chevron = expanded ? ChevronDown : ChevronRight
                    return (
                      <Fragment key={run.traceId}>
                        <TableRow onClick={() => toggle(run)} className="cursor-pointer" aria-expanded={expanded}>
                          <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                            <Chevron className="mr-1 inline size-3.5" />
                            {fmt.format(run.startNs / 1e6)}
                          </TableCell>
                          <TableCell className="max-w-56 truncate">{run.service ?? '–'}</TableCell>
                          <TableCell className={num}>
                            {formatCount(run.calls.length)}
                            {run.failed > 0 && (
                              <Badge variant="destructive" className="ml-2">
                                {formatCount(run.failed)} failed
                              </Badge>
                            )}
                          </TableCell>
                          <TableCell className="max-w-56 truncate font-medium">
                            <Names values={run.models} />
                          </TableCell>
                          <TableCell className="max-w-40 truncate text-muted-foreground">
                            <Names values={run.providers} />
                          </TableCell>
                          <TableCell className={num}>{formatSeconds((run.endNs - run.startNs) / 1e9)}</TableCell>
                          <TableCell className={num}>{formatSeconds(run.inCallsNs / 1e9)}</TableCell>
                          <TableCell className={num}>
                            <Tokens value={run.tokensIn} />
                          </TableCell>
                          <TableCell className={num}>
                            <Tokens value={run.tokensOut} />
                          </TableCell>
                        </TableRow>
                        {expanded &&
                          run.calls.map((row) => {
                            const id = callId(row)
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
                                <TableCell />
                                <TableCell className="text-right">
                                  {failed && (
                                    <Badge variant="destructive" title={field(row.fields, K.errorType)}>
                                      error
                                    </Badge>
                                  )}
                                </TableCell>
                                <TableCell className="max-w-56 truncate">
                                  {field(row.fields, K.requestModel) ?? '–'}
                                </TableCell>
                                <TableCell className="max-w-40 truncate text-muted-foreground">
                                  {field(row.fields, K.provider) ?? '–'}
                                </TableCell>
                                <TableCell />
                                <TableCell className={num}>{formatSeconds(row.duration_ns / 1e9)}</TableCell>
                                <TableCell className={num}>
                                  <Tokens value={field(row.fields, K.inputTokens)} />
                                </TableCell>
                                <TableCell className={num}>
                                  <Tokens value={field(row.fields, K.outputTokens)} />
                                </TableCell>
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
                  {formatCount(runs.length)} runs from the latest {formatCount(calls.length)} of{' '}
                  {formatCount(matched)} calls
                  {query.hasNextPage && '; the oldest runs listed may miss their earlier calls'}
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
