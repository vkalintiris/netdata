import { useInfiniteQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { ListFrame, listTimeFormat, NUM } from '@/components/ListFrame'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, unitOf, usePageState } from '@/lib/page-state'
import { groupTraces, type TraceRow } from '@/lib/trace-list'
import { exploreSpans, ROWS_MAX, spansOfTraces } from '@/lib/traces'

function Failed({ count }: { count: number }) {
  if (count === 0) return null
  return (
    <Badge variant="destructive" className="ml-2">
      {formatCount(count)} failed
    </Badge>
  )
}

// Where a trace opens: its page, keeping the list's window and filters for the way back.
export function traceLink(traceId: string, startNs: number, search: URLSearchParams): string {
  const next = new URLSearchParams(search)
  next.delete('span')
  next.delete('cell')
  next.set('at', String(startNs))
  return `/traces/${traceId}?${next}`
}

// One row per trace that holds a call matching the page's filters, newest matches first. A row adds up every model
// call and tool call of its trace, as the trace's page does: the filters choose the traces, not what a row counts.
export function TraceList({ live }: { live: boolean }) {
  const { window, spanQuery } = usePageState()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const query = useInfiniteQuery({
    queryKey: ['traces', window, spanQuery],
    queryFn: async ({ pageParam, signal }) => {
      const matching = await exploreSpans(window, spanQuery, pageParam, signal, ROWS_MAX)
      const ids = [...new Set(matching.rows.items.map((row) => row.trace_id))]
      return { matching, spans: await spansOfTraces(window, ids, signal) }
    },
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.matching.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const matching = query.data?.pages.reduce((n, p) => n + p.matching.rows.items.length, 0) ?? 0
  const spans = useMemo(() => query.data?.pages.flatMap((p) => p.spans) ?? [], [query.data])
  const traces = useMemo(() => groupTraces(spans), [spans])
  const fmt = listTimeFormat(absolute(window))

  return (
    <ListFrame
      error={query.error}
      loading={!query.data}
      empty={traces.length === 0}
      summary={
        <>
          {formatCount(traces.length)} traces, each whole, that hold the latest {formatCount(matching)} of{' '}
          {formatCount(query.data?.pages[0]?.matching.rows.matched)} {unitOf(spanQuery.kinds)} matching the filters
        </>
      }
      hasMore={query.hasNextPage}
      loadingMore={query.isFetchingNextPage}
      onMore={() => query.fetchNextPage()}
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-36" title="When the trace's first model call or tool call started">
              First call
            </TableHead>
            <TableHead>Service</TableHead>
            <TableHead className="text-right">Model calls</TableHead>
            <TableHead className="text-right">Tool calls</TableHead>
            <TableHead>Models</TableHead>
            <TableHead>Providers</TableHead>
            <TableHead className="text-right" title="From the first model call or tool call to the end of the last">
              First to last call
            </TableHead>
            <TableHead className="text-right" title="Added up; calls made in parallel count each">
              In model calls
            </TableHead>
            <TableHead className="text-right">Tokens in</TableHead>
            <TableHead className="text-right">Tokens out</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {traces.map((trace: TraceRow) => (
            <TableRow
              key={trace.traceId}
              onClick={() => navigate(traceLink(trace.traceId, trace.startNs, params))}
              className="cursor-pointer"
            >
              <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                {fmt.format(trace.startNs / 1e6)}
              </TableCell>
              <TableCell className="max-w-56 truncate">{trace.service ?? '–'}</TableCell>
              <TableCell className={NUM}>
                {formatCount(trace.calls)}
                <Failed count={trace.failedCalls} />
              </TableCell>
              <TableCell className={NUM}>
                {trace.tools ? formatCount(trace.tools) : '–'}
                <Failed count={trace.failedTools} />
              </TableCell>
              <TableCell className="max-w-56 truncate font-medium" title={trace.models.join(', ')}>
                {trace.models.join(', ') || '–'}
              </TableCell>
              <TableCell className="max-w-40 truncate text-muted-foreground" title={trace.providers.join(', ')}>
                {trace.providers.join(', ') || '–'}
              </TableCell>
              <TableCell className={NUM}>{formatSeconds((trace.endNs - trace.startNs) / 1e9)}</TableCell>
              <TableCell className={NUM}>{formatSeconds(trace.inCallsNs / 1e9)}</TableCell>
              <TableCell className={NUM}>{formatCount(trace.tokensIn)}</TableCell>
              <TableCell className={NUM}>{formatCount(trace.tokensOut)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </ListFrame>
  )
}
