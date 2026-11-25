import { useInfiniteQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { Distribution } from '@/components/Distribution'
import { ListFrame, listTimeFormat, NUM } from '@/components/ListFrame'
import { Panel, SectionTitle } from '@/components/Panel'
import { Ranking, type RankRow } from '@/components/Ranking'
import { StatTile } from '@/components/StatTile'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { traceLink } from '@/components/TraceList'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { totals, type DataQuery, type Series } from '@/lib/agent'
import { C, durationSeconds, F, L, logSelections, NARROW, statusClass, type Narrow } from '@/lib/agentgateway'
import { bucketTotals, quantileLines } from '@/lib/charts'
import { formatCount, formatNumber, formatPercent, formatSeconds, formatUsd } from '@/lib/format'
import { fetchLogsPage } from '@/lib/logs'
import { absolute, POINTS, useChartNav, usePageState } from '@/lib/page-state'
import { quantile } from '@/lib/quantile'
import { useSeries } from '@/lib/queries'

const CLASS_COLORS: Record<string, string> = { '2xx': '--success', '4xx': '--chart-4', '5xx': '--error' }
const LATENCY_STYLES: SeriesStyle[] = [
  { label: 'request', name: 'Whole request', color: '--chart-1' },
  { label: 'upstream', name: 'Provider call', color: '--chart-3' },
]
const TOKEN_STYLES: SeriesStyle[] = [
  { label: 'input', name: 'Input', color: '--chart-1' },
  { label: 'output', name: 'Output', color: '--chart-2', right: true },
  { label: 'input_cache_read', name: 'Cache read', color: '--chart-3' },
]

const fmtRate = (v: number | null) => formatNumber(v)
const fmtSeconds = (v: number | null) => formatSeconds(v)

function rows(s: Series | undefined): RankRow[] {
  if (!s) return []
  return Object.entries(totals(s))
    .filter(([, value]) => value > 0)
    .map(([key, value]) => ({ key, name: key, value }))
}

// One style per status code the chart has, coloured by its class.
function statusStyles(s: Series | undefined): SeriesStyle[] {
  return (s?.labels ?? []).map((code) => ({ label: code, name: code, color: CLASS_COLORS[statusClass(code)] ?? '--chart-6' }))
}

// agentgateway: what it handled (its Prometheus metrics, as go.d's generic charts), each request it answered (its
// OTLP access logs) and, from a request, its trace. The OTel GenAI pages count what applications report; this page
// counts what the gateway saw, including requests it rejected.
export function AgentgatewayPage() {
  const { window } = usePageState()
  const [params, setParams] = useSearchParams()
  const nav = useChartNav()

  const narrow: Narrow = useMemo(() => {
    const out: Narrow = {}
    for (const [name, param] of Object.entries(NARROW)) {
      const v = params.get(param)
      if (v) out[name as keyof Narrow] = v
    }
    return out
  }, [params])
  const toggle = (name: keyof Narrow) => (value: string) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (next.get(NARROW[name]) === value) next.delete(NARROW[name])
      else next.set(NARROW[name], value)
      return next
    })

  const total = (context: string, extra: Partial<DataQuery> = {}): DataQuery => ({
    window,
    context,
    points: 1,
    timeGroup: 'sum',
    aggregation: 'sum',
    ...extra,
  })
  // Rates per second, averaged over each point; totals are sums over the window, which the Agent answers as counts.
  const trend = (context: string, extra: Partial<DataQuery> = {}): DataQuery => ({
    window,
    context,
    points: POINTS,
    timeGroup: 'average',
    aggregation: 'sum',
    ...extra,
  })

  const byStatus = useSeries(total(C.requests, { groupByLabels: [L.status] }))
  const statusTrend = useSeries(trend(C.requests, { groupByLabels: [L.status] }))
  const failures = useSeries(total(C.requests, { groupByLabels: [L.status, L.reason] }))
  const spendByTeam = useSeries(total(C.cost, { groupByLabels: [L.team] }))
  const spendByModel = useSeries(total(C.cost, { groupByLabels: [L.model] }))
  const tokensByType = useSeries(total(C.tokens, { groupByLabels: [L.tokenType] }))
  const tokensTrend = useSeries(trend(C.tokens, { groupByLabels: [L.tokenType] }))
  const tokensByModel = useSeries(total(C.tokens, { groupByLabels: [L.model], filters: { [L.tokenType]: ['input', 'output'] } }))
  const catalog = useSeries(total(C.catalog, { groupByLabels: [L.catalogStatus] }))
  const durationH = useSeries(total(C.duration))
  const ttftH = useSeries(total(C.ttft))
  const durationTrendH = useSeries(trend(C.duration))
  const upstreamTrendH = useSeries(trend(C.upstream, { filters: { [L.subtype]: ['Llm'] } }))
  const mcpByTool = useSeries(total(C.mcp, { groupByLabels: [L.mcpServer, L.mcpResource] }))
  const mcpByMethod = useSeries(total(C.mcp, { groupByLabels: [L.mcpMethod] }))

  const statusCounts = byStatus.data ? totals(byStatus.data) : undefined
  const requestCount = statusCounts ? Object.values(statusCounts).reduce((a, b) => a + b, 0) : undefined
  const failed = statusCounts
    ? Object.entries(statusCounts).reduce((sum, [code, n]) => (statusClass(code) === '2xx' ? sum : sum + n), 0)
    : undefined
  const spend = spendByModel.data ? Object.values(totals(spendByModel.data)).reduce((a, b) => a + b, 0) : undefined
  const tokens = tokensByType.data ? totals(tokensByType.data) : undefined
  const lookups = catalog.data ? totals(catalog.data) : undefined
  const unpriced = lookups ? (lookups.Missing ?? 0) + (lookups.Unpriced ?? 0) + (lookups.NoCatalog ?? 0) : undefined
  const durationBuckets = bucketTotals(durationH.data)
  const ttftBuckets = bucketTotals(ttftH.data)
  const q = (b: Record<string, number> | undefined, p: number) => (b ? quantile(p, b) : null)

  const p95 = useMemo(
    () =>
      quantileLines(0.95, [
        { label: 'request', series: durationTrendH.data },
        { label: 'upstream', series: upstreamTrendH.data },
      ]),
    [durationTrendH.data, upstreamTrendH.data],
  )
  const failureRows = rows(failures.data).filter((r) => statusClass(r.name.split(',')[0]) !== '2xx')
  const mcpRows = rows(mcpByTool.data)
  const noMetrics = byStatus.data && byStatus.data.labels.length === 0

  return (
    <div className="flex flex-col">
      <div className="flex flex-wrap items-baseline gap-x-3">
        <h1 className="text-lg font-semibold">agentgateway</h1>
        <p className="text-sm text-muted-foreground">
          What the gateway handled, from its Prometheus metrics, and each request it answered, from its access logs.
        </p>
      </div>
      {noMetrics && (
        <p className="mt-2 text-sm text-muted-foreground">
          No agentgateway metrics in this window: the charts need go.d's prometheus collector scraping its stats port.
        </p>
      )}

      <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
        <StatTile label="Requests" value={formatCount(requestCount)} loading={!byStatus.data} detail="in this window, rejected ones included" />
        <StatTile
          label="Failed"
          value={formatPercent(requestCount ? (failed ?? 0) / requestCount : undefined)}
          loading={!byStatus.data}
          tone={failed ? 'error' : undefined}
          detail={failed != null ? `${formatCount(failed)} requests not 2xx` : undefined}
        />
        <StatTile
          label="Spend"
          value={formatUsd(spend)}
          loading={!spendByModel.data}
          tone={unpriced ? 'error' : undefined}
          detail={unpriced ? `${formatCount(unpriced)} calls unpriced, so higher` : 'priced from its model catalog'}
        />
        <StatTile
          label="Tokens"
          value={formatCount(tokens ? (tokens.input ?? 0) + (tokens.output ?? 0) : undefined)}
          loading={!tokensByType.data}
          detail={tokens ? `${formatCount(tokens.input)} in · ${formatCount(tokens.output)} out · ${formatCount(tokens.input_cache_read)} cached` : undefined}
        />
        <StatTile
          label="Request duration p50"
          value={formatSeconds(q(durationBuckets, 0.5))}
          loading={!durationH.data}
          detail={`p95 ${formatSeconds(q(durationBuckets, 0.95))}`}
        />
        <StatTile
          label="Time to first token p50"
          value={formatSeconds(q(ttftBuckets, 0.5))}
          loading={!ttftH.data}
          detail={q(ttftBuckets, 0.5) == null ? 'no streamed calls' : `p95 ${formatSeconds(q(ttftBuckets, 0.95))}`}
        />
      </div>

      <SectionTitle>Traffic</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Panel title="Requests by status" subtitle="requests per second">
          <TimeChart data={statusTrend.data} styles={statusStyles(statusTrend.data)} stacked format={fmtRate} {...nav} />
        </Panel>
        <Panel title="Latency p95" subtitle="whole request and the call to the provider">
          <TimeChart data={p95} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="Request duration distribution" subtitle="model calls per duration">
          <Distribution buckets={durationBuckets} />
        </Panel>
      </div>

      <SectionTitle>Spend and tokens</SectionTitle>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
        <Panel title="Spend by team" subtitle="click to list the team's requests">
          <Ranking
            rows={rows(spendByTeam.data)}
            format={formatUsd}
            empty="No spend by a team"
            selected={narrow.team ? [narrow.team] : []}
            onSelect={toggle('team')}
          />
        </Panel>
        <Panel title="Spend by model" subtitle="requested model">
          <Ranking
            rows={rows(spendByModel.data)}
            format={formatUsd}
            empty="No spend"
            selected={narrow.model ? [narrow.model] : []}
            onSelect={toggle('model')}
          />
        </Panel>
        <Panel title="Tokens by model" subtitle="input and output">
          <Ranking
            rows={rows(tokensByModel.data)}
            format={formatCount}
            empty="No tokens"
            selected={narrow.model ? [narrow.model] : []}
            onSelect={toggle('model')}
          />
        </Panel>
        <Panel title="Failed requests" subtitle="status and reason; click to list them">
          <Ranking
            rows={failureRows}
            format={formatCount}
            empty="No failed requests"
            selected={narrow.status ? failureRows.filter((r) => r.name.startsWith(`${narrow.status},`)).map((r) => r.name) : []}
            onSelect={(name) => toggle('status')(name.split(',')[0])}
          />
        </Panel>
        <Panel title="Tokens" subtitle="tokens per second" className="md:col-span-2 xl:col-span-4">
          <TimeChart data={tokensTrend.data} styles={TOKEN_STYLES} format={fmtRate} {...nav} />
        </Panel>
      </div>

      <SectionTitle>MCP tools</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
        <Panel title="Tool calls by server and tool" subtitle="MCP requests through the gateway">
          <Ranking rows={mcpRows} format={formatCount} empty="No MCP traffic in this window" selected={[]} onSelect={() => {}} />
        </Panel>
        <Panel title="MCP requests by method" subtitle="tools/call, tools/list, initialize, ...">
          <Ranking rows={rows(mcpByMethod.data)} format={formatCount} empty="No MCP traffic in this window" selected={[]} onSelect={() => {}} />
        </Panel>
      </div>

      <SectionTitle>Access log</SectionTitle>
      <AccessLog narrow={narrow} onClear={(name) => toggle(name)(narrow[name] ?? '')} />
    </div>
  )
}

// The gateway's access log: one record per request, newest first. A record with a trace opens it.
function AccessLog({ narrow, onClear }: { narrow: Narrow; onClear: (name: keyof Narrow) => void }) {
  const { window } = usePageState()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const live = window.after < 0
  const selections = useMemo(() => logSelections(narrow), [narrow])
  const query = useInfiniteQuery({
    queryKey: ['agentgateway-logs', window, selections],
    queryFn: ({ pageParam, signal }) => fetchLogsPage(absolute(window), selections, pageParam, 100, signal),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const records = useMemo(() => query.data?.pages.flatMap((p) => p.records) ?? [], [query.data])
  const fmt = listTimeFormat(absolute(window))
  const chips = (Object.keys(narrow) as (keyof Narrow)[]).map((name) => (
    <Badge key={name} variant="secondary" className="cursor-pointer" onClick={() => onClear(name)}>
      {name}: {narrow[name]} ✕
    </Badge>
  ))

  return (
    <ListFrame
      error={query.error}
      loading={!query.data}
      empty={records.length === 0}
      summary={
        <span className="flex flex-wrap items-center gap-2">
          {`${formatCount(records.length)} of ${formatCount(query.data?.pages[0]?.matched)} requests, newest first`}
          {chips}
        </span>
      }
      hasMore={query.hasNextPage}
      loadingMore={query.isFetchingNextPage}
      onMore={() => query.fetchNextPage()}
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-36">Time</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Team</TableHead>
            <TableHead>Model or tool</TableHead>
            <TableHead>Path</TableHead>
            <TableHead className="text-right">Duration</TableHead>
            <TableHead className="text-right">Tokens in</TableHead>
            <TableHead className="text-right">Tokens out</TableHead>
            <TableHead className="text-right">Cost</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {records.map((r) => {
            const status = r[F.status] ?? '–'
            const traceId = r[F.traceId]
            const startNs = Number(r[F.timestamp]) * 1000
            return (
              <TableRow
                key={r[F.cursor]}
                className={traceId ? 'cursor-pointer' : undefined}
                onClick={() => {
                  if (!traceId) return
                  const search = new URLSearchParams(params)
                  search.set('from', 'agentgateway')
                  navigate(traceLink(traceId, startNs, search))
                }}
              >
                <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                  {fmt.format(Number(r[F.timestamp]) / 1000)}
                </TableCell>
                <TableCell>
                  {statusClass(status) === '2xx' ? (
                    <span className="text-xs text-muted-foreground">{status}</span>
                  ) : (
                    <Badge variant="destructive">{status}</Badge>
                  )}
                </TableCell>
                <TableCell>{r[F.team] ?? '–'}</TableCell>
                <TableCell className="font-medium">{r[F.model] ?? r[F.tool] ?? r[F.mcpTarget] ?? '–'}</TableCell>
                <TableCell className="font-mono text-xs text-muted-foreground">{r[F.path] ?? '–'}</TableCell>
                <TableCell className={NUM}>{formatSeconds(durationSeconds(r[F.duration]))}</TableCell>
                <TableCell className={NUM}>{r[F.inputTokens] ? formatCount(Number(r[F.inputTokens])) : '–'}</TableCell>
                <TableCell className={NUM}>{r[F.outputTokens] ? formatCount(Number(r[F.outputTokens])) : '–'}</TableCell>
                <TableCell className={NUM}>{r[F.cost] ? formatUsd(Number(r[F.cost])) : '–'}</TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </ListFrame>
  )
}
