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
import { formatCount, formatNumber, formatPercent, formatSeconds, formatUsd } from '@/lib/format'
import {
  bucketTotals,
  C,
  deploymentState,
  exploreRequests,
  L,
  latest,
  NARROW,
  quantileLines,
  S,
  type Narrow,
} from '@/lib/litellm'
import { absolute, POINTS, useChartNav, usePageState } from '@/lib/page-state'
import { quantile } from '@/lib/quantile'
import { useSeries } from '@/lib/queries'
import { field, K } from '@/lib/traces'

const STATUS_STYLES: SeriesStyle[] = [
  { label: '2xx', name: '2xx', color: '--success' },
  { label: '4xx', name: '4xx', color: '--chart-4' },
  { label: '5xx', name: '5xx', color: '--error' },
  { label: 'unclassified', name: 'Unclassified', color: '--chart-6' },
]
const LATENCY_STYLES: SeriesStyle[] = [
  { label: 'request', name: 'Whole request', color: '--chart-1' },
  { label: 'provider', name: 'Provider API', color: '--chart-3' },
  { label: 'overhead', name: 'LiteLLM overhead', color: '--chart-4', right: true },
]
const TOKEN_STYLES: SeriesStyle[] = [
  { label: 'input', name: 'Input', color: '--chart-1' },
  { label: 'output', name: 'Output', color: '--chart-2', right: true },
]

const fmtRate = (v: number | null) => formatNumber(v)
const fmtSeconds = (v: number | null) => formatSeconds(v)

function rows(s: Series | undefined, counts = true): RankRow[] {
  if (!s) return []
  const t = counts ? totals(s) : latest(s)
  return Object.entries(t)
    .filter(([, value]) => value > 0)
    .map(([key, value]) => ({ key, name: key, value }))
}

// The LiteLLM proxy: what it handled (its Prometheus metrics, charted by go.d's stock LiteLLM profile) and each
// request it forwarded (its spans). The OTel GenAI pages count what applications report; this page counts what the
// gateway saw, including callers that send no telemetry.
export function LiteLLMPage() {
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

  const byStatus = useSeries(total(C.requests, { groupByLabels: [L.statusClass] }))
  const statusTrend = useSeries(trend(C.requests, { groupByLabels: [L.statusClass] }))
  const failureCauses = useSeries(total(C.failureCauses, { groupByLabels: [L.exceptionClass] }))
  const spendByModel = useSeries(total(C.spendByModel, { groupByLabels: [L.requestedModel] }))
  const spendByTeam = useSeries(total(C.spendByTeam, { groupByLabels: [L.team] }))
  const spendByKey = useSeries(total(C.spendByKey, { groupByLabels: [L.key] }))
  const tokens = useSeries(total(C.tokensByModel))
  const tokensTrend = useSeries(trend(C.tokensByModel))
  const tokensByModel = useSeries(total(C.tokensByModel, { groupByLabels: [L.requestedModel] }))
  const requestH = useSeries(total(C.requestLatency))
  const overheadH = useSeries(total(C.overhead))
  const ttftH = useSeries(total(C.ttft))
  const requestTrendH = useSeries(trend(C.requestLatency))
  const providerTrendH = useSeries(trend(C.providerLatency))
  const overheadTrendH = useSeries(trend(C.overhead))
  const remaining = useSeries(trend(C.teamRemaining, { groupByLabels: [L.team] }))
  const limit = useSeries(trend(C.teamLimit, { groupByLabels: [L.team] }))
  const state = useSeries(trend(C.deploymentState, { groupByLabels: [L.model], aggregation: 'max' }))
  const deploymentOutcomes = useSeries(
    total(C.deploymentOutcomes, { groupByLabels: [L.model], groupByDimension: true }),
  )
  const cooldowns = useSeries(total(C.cooldowns, { groupByLabels: [L.model] }))

  const statusCounts = byStatus.data ? totals(byStatus.data) : undefined
  const requestCount = statusCounts ? Object.values(statusCounts).reduce((a, b) => a + b, 0) : undefined
  const failed = statusCounts
    ? Object.entries(statusCounts).reduce((sum, [status, n]) => (status === '2xx' ? sum : sum + n), 0)
    : undefined
  const spend = spendByModel.data ? Object.values(totals(spendByModel.data)).reduce((a, b) => a + b, 0) : undefined
  const tokenTotals = tokens.data ? totals(tokens.data) : undefined
  const requestBuckets = bucketTotals(requestH.data)
  const overheadBuckets = bucketTotals(overheadH.data)
  const ttftBuckets = bucketTotals(ttftH.data)
  const q = (b: Record<string, number> | undefined, p: number) => (b ? quantile(p, b) : null)

  const p95 = useMemo(
    () =>
      quantileLines(0.95, [
        { label: 'request', series: requestTrendH.data },
        { label: 'provider', series: providerTrendH.data },
        { label: 'overhead', series: overheadTrendH.data },
      ]),
    [requestTrendH.data, providerTrendH.data, overheadTrendH.data],
  )

  const budgets = useMemo(() => {
    const left = latest(remaining.data)
    const cap = latest(limit.data)
    return Object.keys({ ...left, ...cap })
      .sort()
      .map((team) => ({ team, remaining: left[team], limit: cap[team] }))
  }, [remaining.data, limit.data])

  const deployments = useMemo(() => {
    const now = latest(state.data)
    const outcomes = deploymentOutcomes.data ? totals(deploymentOutcomes.data) : {}
    const cooled = cooldowns.data ? totals(cooldowns.data) : {}
    const models = new Set(Object.keys(now))
    for (const key of Object.keys(outcomes)) models.add(key.split(',').slice(1).join(','))
    return [...models].sort().map((model) => ({
      model,
      state: deploymentState(now[model]),
      successful: outcomes[`successful,${model}`] ?? 0,
      failed: outcomes[`failed,${model}`] ?? 0,
      cooldowns: cooled[model] ?? 0,
    }))
  }, [state.data, deploymentOutcomes.data, cooldowns.data])

  const noMetrics = byStatus.data && byStatus.data.labels.length === 0

  return (
    <div className="flex flex-col">
      <div className="flex flex-wrap items-baseline gap-x-3">
        <h1 className="text-lg font-semibold">LiteLLM</h1>
        <p className="text-sm text-muted-foreground">
          What the proxy handled, from its Prometheus metrics, and each request it forwarded, from its spans.
        </p>
      </div>
      {noMetrics && (
        <p className="mt-2 text-sm text-muted-foreground">
          No LiteLLM metrics in this window: the charts need go.d's prometheus collector scraping the proxy's /metrics.
        </p>
      )}

      <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
        <StatTile label="Requests" value={formatCount(requestCount)} loading={!byStatus.data} detail="in this window" />
        <StatTile
          label="Failed"
          value={formatPercent(requestCount ? (failed ?? 0) / requestCount : undefined)}
          loading={!byStatus.data}
          tone={failed ? 'error' : undefined}
          detail={failed != null ? `${formatCount(failed)} requests not 2xx` : undefined}
        />
        <StatTile label="Spend" value={formatUsd(spend)} loading={!spendByModel.data} detail="as LiteLLM prices it" />
        <StatTile
          label="Tokens"
          value={formatCount(tokenTotals ? (tokenTotals.input ?? 0) + (tokenTotals.output ?? 0) : undefined)}
          loading={!tokens.data}
          detail={tokenTotals ? `${formatCount(tokenTotals.input)} in · ${formatCount(tokenTotals.output)} out` : undefined}
        />
        <StatTile
          label="Request latency p50"
          value={formatSeconds(q(requestBuckets, 0.5))}
          loading={!requestH.data}
          detail={`p95 ${formatSeconds(q(requestBuckets, 0.95))} · first token p50 ${formatSeconds(q(ttftBuckets, 0.5))}`}
        />
        <StatTile
          label="LiteLLM overhead p95"
          value={formatSeconds(q(overheadBuckets, 0.95))}
          loading={!overheadH.data}
          detail="time the proxy itself adds"
        />
      </div>

      <SectionTitle>Traffic</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Panel title="Requests by status" subtitle="requests per second">
          <TimeChart data={statusTrend.data} styles={STATUS_STYLES} stacked format={fmtRate} {...nav} />
        </Panel>
        <Panel title="Latency p95" subtitle="whole request, provider API and the proxy's own share">
          <TimeChart data={p95} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="Request latency distribution" subtitle="requests per duration">
          <Distribution buckets={requestBuckets} />
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
        <Panel title="Spend by key" subtitle="click to list the key's requests">
          <Ranking
            rows={rows(spendByKey.data)}
            format={formatUsd}
            empty="No spend by a key"
            selected={narrow.key ? [narrow.key] : []}
            onSelect={toggle('key')}
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
        <Panel title="Tokens" subtitle="tokens per second" className="md:col-span-2">
          <TimeChart data={tokensTrend.data} styles={TOKEN_STYLES} format={fmtRate} {...nav} />
        </Panel>
        <Panel title="Failure causes" subtitle="requests that failed, by exception" className="md:col-span-2">
          <Ranking rows={rows(failureCauses.data)} format={formatCount} empty="No failed requests" selected={[]} onSelect={() => {}} />
        </Panel>
      </div>

      <SectionTitle>Budgets and deployments</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
        <Panel title="Team budgets" subtitle="spent of the budget, now">
          <BudgetTable budgets={budgets} />
        </Panel>
        <Panel title="Deployments" subtitle="state now; requests and cooldowns in this window">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Model</TableHead>
                <TableHead>State</TableHead>
                <TableHead className="text-right">Succeeded</TableHead>
                <TableHead className="text-right">Failed</TableHead>
                <TableHead className="text-right">Cooldowns</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {deployments.map((d) => (
                <TableRow key={d.model}>
                  <TableCell className="font-medium">{d.model}</TableCell>
                  <TableCell>
                    {d.state.tone ? <Badge variant="destructive">{d.state.text}</Badge> : <span className="text-xs">{d.state.text}</span>}
                  </TableCell>
                  <TableCell className={NUM}>{formatCount(d.successful)}</TableCell>
                  <TableCell className={NUM}>{formatCount(d.failed)}</TableCell>
                  <TableCell className={NUM}>{formatCount(d.cooldowns)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Panel>
      </div>

      <SectionTitle>Requests</SectionTitle>
      <RequestList narrow={narrow} onClear={(name) => toggle(name)(narrow[name] ?? '')} />
    </div>
  )
}

function BudgetTable({ budgets }: { budgets: { team: string; remaining?: number; limit?: number }[] }) {
  if (!budgets.length) return <p className="text-sm text-muted-foreground">No team has a budget.</p>
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Team</TableHead>
          <TableHead className="w-1/2">Spent</TableHead>
          <TableHead className="text-right">Left</TableHead>
          <TableHead className="text-right">Budget</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {budgets.map((b) => {
          const spent = b.limit != null && b.remaining != null ? b.limit - b.remaining : undefined
          const share = spent != null && b.limit ? Math.min(1, Math.max(0, spent / b.limit)) : 0
          return (
            <TableRow key={b.team}>
              <TableCell className="font-medium">{b.team}</TableCell>
              <TableCell>
                <div className="flex items-center gap-2">
                  <div className="h-2 flex-1 rounded bg-muted">
                    <div
                      className={share > 0.9 ? 'h-2 rounded bg-[var(--error)]' : 'h-2 rounded bg-[var(--chart-1)]'}
                      style={{ width: `${Math.max(share * 100, share > 0 ? 1 : 0)}%` }}
                    />
                  </div>
                  <span className="w-20 text-right font-mono text-xs">{formatUsd(spent)}</span>
                </div>
              </TableCell>
              <TableCell className={NUM}>{formatUsd(b.remaining)}</TableCell>
              <TableCell className={NUM}>{formatUsd(b.limit)}</TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

// The proxy's own record of each request: its call spans, newest first. A row opens the request's trace.
function RequestList({ narrow, onClear }: { narrow: Narrow; onClear: (name: keyof Narrow) => void }) {
  const { window } = usePageState()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const live = window.after < 0
  const query = useInfiniteQuery({
    queryKey: ['litellm-requests', window, narrow],
    queryFn: ({ pageParam, signal }) => exploreRequests(window, narrow, pageParam, signal),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.rows.anchor?.next,
    refetchInterval: live ? 10_000 : false,
    placeholderData: (prev) => prev,
  })
  const items = useMemo(() => query.data?.pages.flatMap((p) => p.rows.items) ?? [], [query.data])
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
      empty={items.length === 0}
      summary={
        <span className="flex flex-wrap items-center gap-2">
          {`${formatCount(items.length)} of ${formatCount(query.data?.pages[0]?.rows.matched)} requests, newest first`}
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
            <TableHead>Team</TableHead>
            <TableHead>Key</TableHead>
            <TableHead>Model</TableHead>
            <TableHead>Status</TableHead>
            <TableHead className="text-right">Duration</TableHead>
            <TableHead className="text-right">Tokens in</TableHead>
            <TableHead className="text-right">Tokens out</TableHead>
            <TableHead className="text-right">Cost</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {items.map((row) => {
            const cost = field(row.fields, S.cost)
            const tokensIn = field(row.fields, K.inputTokens)
            const tokensOut = field(row.fields, K.outputTokens)
            return (
              <TableRow
                key={`${row.trace_id}:${row.span_id}`}
                className="cursor-pointer"
                onClick={() => {
                  const search = new URLSearchParams(params)
                  search.set('from', 'litellm')
                  navigate(traceLink(row.trace_id, row.start_ns, search))
                }}
              >
                <TableCell className="font-mono text-xs whitespace-nowrap text-muted-foreground">
                  {fmt.format(row.start_ns / 1e6)}
                </TableCell>
                <TableCell>{field(row.fields, S.team) ?? '–'}</TableCell>
                <TableCell className="text-muted-foreground">{field(row.fields, S.key) ?? '–'}</TableCell>
                <TableCell className="font-medium">{field(row.fields, K.requestModel) ?? '–'}</TableCell>
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
                <TableCell className={NUM}>{tokensIn ? formatCount(Number(tokensIn)) : '–'}</TableCell>
                <TableCell className={NUM}>{tokensOut ? formatCount(Number(tokensOut)) : '–'}</TableCell>
                <TableCell className={NUM}>{cost ? formatUsd(Number(cost)) : '–'}</TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </ListFrame>
  )
}
