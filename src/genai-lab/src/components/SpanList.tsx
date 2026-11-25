import { useInfiniteQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { ListFrame, listTimeFormat, NUM } from '@/components/ListFrame'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatCount, formatSeconds } from '@/lib/format'
import { absolute, usePageState } from '@/lib/page-state'
import { exploreSpans, field, K, type CallCell, type SpanRow } from '@/lib/traces'

export function spanKey(row: { trace_id: string; span_id: string; start_ns: number }): string {
  return `${row.trace_id}:${row.span_id}:${row.start_ns}`
}

// Model calls and tool calls, one row each, newest first.
export function SpanList({
  cell,
  selected,
  onSelect,
  live,
}: {
  cell: CallCell | null
  selected: string | null
  onSelect: (row: SpanRow | null) => void
  live: boolean
}) {
  const { window, spanQuery } = usePageState()
  const query = useInfiniteQuery({
    queryKey: ['spans', window, spanQuery, cell],
    queryFn: ({ pageParam, signal }) => exploreSpans(window, spanQuery, pageParam, signal, cell),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
  const fmt = listTimeFormat(absolute(window))

  return (
    <ListFrame
      error={query.error}
      loading={!query.data}
      empty={rows.length === 0}
      summary={`${formatCount(rows.length)} of ${formatCount(query.data?.pages[0]?.rows.matched)} spans, newest first`}
      hasMore={query.hasNextPage}
      loadingMore={query.isFetchingNextPage}
      onMore={() => query.fetchNextPage()}
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-36">Time</TableHead>
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
            const id = spanKey(row)
            const active = selected === id
            const tool = field(row.fields, K.kind) === 'tool'
            const tokensIn = field(row.fields, K.inputTokens)
            const tokensOut = field(row.fields, K.outputTokens)
            return (
              <TableRow
                key={id}
                onClick={() => onSelect(active ? null : row)}
                data-state={active ? 'selected' : undefined}
                className="cursor-pointer"
              >
                <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                  {fmt.format(row.start_ns / 1e6)}
                </TableCell>
                <TableCell className="max-w-56 truncate">{field(row.fields, K.service)}</TableCell>
                <TableCell className="text-xs whitespace-nowrap text-muted-foreground">
                  {tool ? 'tool call' : 'model call'}
                </TableCell>
                <TableCell className="max-w-56 truncate font-medium">
                  {(tool ? field(row.fields, K.tool, K.name) : field(row.fields, K.requestModel)) ?? '–'}
                </TableCell>
                <TableCell className="text-muted-foreground">{tool ? '' : (field(row.fields, K.provider) ?? '–')}</TableCell>
                <TableCell>
                  {field(row.fields, K.outcome) === 'error' ? (
                    <Badge variant="destructive" title={field(row.fields, K.errorType)}>
                      error
                    </Badge>
                  ) : (
                    <span className="text-xs text-muted-foreground">ok</span>
                  )}
                </TableCell>
                <TableCell className={NUM}>{formatSeconds(row.duration_ns / 1e9)}</TableCell>
                <TableCell className={NUM}>{tokensIn ? formatCount(Number(tokensIn)) : tool ? '' : '–'}</TableCell>
                <TableCell className={NUM}>{tokensOut ? formatCount(Number(tokensOut)) : tool ? '' : '–'}</TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </ListFrame>
  )
}
