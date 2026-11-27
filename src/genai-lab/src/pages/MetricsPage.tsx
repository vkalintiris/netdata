import { useMemo } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { Distribution } from '@/components/Distribution'
import { Heatmap, type HeatCell, type HeatmapData } from '@/components/Heatmap'
import { FilterPanels } from '@/components/FilterPanels'
import { Panel, SectionTitle } from '@/components/Panel'
import { Ranking, type RankRow } from '@/components/Ranking'
import { StatTile } from '@/components/StatTile'
import { ToolsSection } from '@/components/ToolsSection'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { totals, type DataQuery, type Series } from '@/lib/agent'
import { LATENCY_STYLES, latencyLines, OUTCOME_STYLES } from '@/lib/charts'
import { formatCount, formatInterval, formatNumber, formatPercent, formatSeconds } from '@/lib/format'
import { LABELS, POINTS, useChartNav, usePageState, type FilterName } from '@/lib/page-state'
import { quantile } from '@/lib/quantile'
import { useSeries } from '@/lib/queries'
import { cellParam } from '@/lib/page-state'

const C = {
  calls: 'otel.derived_metrics.genai.calls',
  tokens: 'otel.derived_metrics.genai.tokens',
  details: 'otel.derived_metrics.genai.token_details',
  duration: 'otel.derived_metrics.genai.call_duration',
  ttfc: 'otel.derived_metrics.genai.time_to_first_chunk',
}


const TOKEN_STYLES: SeriesStyle[] = [
  { label: 'input', name: 'Input', color: '--chart-1' },
  { label: 'output', name: 'Output', color: '--chart-2', right: true },
]
const DETAIL_STYLES: SeriesStyle[] = [
  { label: 'cache_read', name: 'Cache read', color: '--chart-3' },
  { label: 'cache_write', name: 'Cache write', color: '--chart-4', right: true },
  { label: 'reasoning', name: 'Reasoning', color: '--chart-6', right: true },
]

const fmtCount = (v: number | null) => formatCount(v)
const fmtNumber = (v: number | null) => formatNumber(v)
const fmtSeconds = (v: number | null) => formatSeconds(v)

function rankRows(s: Series | undefined): RankRow[] {
  if (!s) return []
  const t = totals(s)
  return Object.entries(t).map(([key, value]) => {
    const [name, group] = key.split(',')
    return { key, name, group, value }
  })
}

export function MetricsPage() {
  const { window, filters, selected, toggleFilter } = usePageState()
  const navigate = useNavigate()
  const [params] = useSearchParams()

  // The calls behind a heatmap cell, on the Traces tab. The chart counts a call when it ends and the list finds it
  // by its start, so the time range reaches back by the longest duration of the cell.
  const listCalls = (c: HeatCell, d: HeatmapData) => {
    const row = d.rows[c.row]
    const before = d.times[c.col] + d.step
    const after = d.times[c.col] - Math.ceil(row.upper ?? 600)
    const next = new URLSearchParams(params)
    // The cell counts model calls under this page's filters only.
    for (const key of ['kind', 'outcome', 'where']) next.delete(key)
    next.append('kind', 'model call')
    next.set('cell', cellParam({ after, before, min: row.lower, max: row.upper }))
    navigate(`/traces?${next}`)
  }

  const base = useMemo(() => ({ window, filters }), [window, filters])
  const total = (context: string, extra: Partial<DataQuery> = {}): DataQuery => ({
    ...base,
    context,
    points: 1,
    timeGroup: 'sum',
    aggregation: 'sum',
    ...extra,
  })
  const trend = (context: string, extra: Partial<DataQuery> = {}): DataQuery => ({
    ...base,
    context,
    points: POINTS,
    timeGroup: 'sum',
    aggregation: 'sum',
    ...extra,
  })

  const calls = useSeries(total(C.calls))
  const tokens = useSeries(total(C.tokens))
  const durationH = useSeries(total(`${C.duration}_histogram`))
  const ttfcH = useSeries(total(`${C.ttfc}_histogram`))

  const callsTrend = useSeries(trend(C.calls))
  const tokensTrend = useSeries(trend(C.tokens))
  const detailsTrend = useSeries(trend(C.details))
  const durationTrendH = useSeries(trend(`${C.duration}_histogram`))
  const durationMax = useSeries(trend(`${C.duration}_max`, { timeGroup: 'max', aggregation: 'max' }))
  const ttfcTrendH = useSeries(trend(`${C.ttfc}_histogram`))
  const ttfcMax = useSeries(trend(`${C.ttfc}_max`, { timeGroup: 'max', aggregation: 'max' }))

  const by = (context: string, labels: string[]) => total(context, { groupByLabels: labels })
  const tokensByModel = useSeries(by(C.tokens, [LABELS.model, LABELS.provider]))
  const failuresByModel = useSeries(total(C.calls, { groupByLabels: [LABELS.model, LABELS.provider], dimensions: ['error'] }))
  const tokensByService = useSeries(by(C.tokens, [LABELS.service]))

  const durationLines = useMemo(() => latencyLines(durationTrendH.data, durationMax.data), [durationTrendH.data, durationMax.data])
  const durationHeat = useMemo(() => metricHeatmap(durationTrendH.data), [durationTrendH.data])
  const ttfcHeat = useMemo(() => metricHeatmap(ttfcTrendH.data), [ttfcTrendH.data])
  const ttfcLines = useMemo(() => latencyLines(ttfcTrendH.data, ttfcMax.data), [ttfcTrendH.data, ttfcMax.data])

  const callTotals = calls.data ? totals(calls.data) : undefined
  const tokenTotals = tokens.data ? totals(tokens.data) : undefined
  const durationBuckets = durationH.data ? totals(durationH.data) : undefined
  const ttfcBuckets = ttfcH.data ? totals(ttfcH.data) : undefined

  const callCount = callTotals ? (callTotals.success ?? 0) + (callTotals.error ?? 0) : undefined
  const errorRate = callTotals && callCount ? (callTotals.error ?? 0) / callCount : undefined
  const successCount = callTotals?.success ?? 0
  const q = (b: Record<string, number> | undefined, p: number) => (b ? quantile(p, b) : null)

  const nav = useChartNav()

  const per = (s: Series | undefined) => (s?.pointSeconds ? `per ${formatInterval(s.pointSeconds)}` : undefined)
  // A model row is a model and its provider; its click filters by both, so the page shows exactly that row.
  const rank = (name: FilterName, groupName?: FilterName) => ({
    selected: selected(name),
    groupSelected: groupName ? selected(groupName) : undefined,
    onSelect: (v: string, group?: string) => {
      const withGroup = groupName != null && group != null
      const active = selected(name).includes(v) && (!withGroup || selected(groupName).includes(group))
      if (active) {
        toggleFilter(name, v)
        if (withGroup) toggleFilter(groupName, group)
        return
      }
      if (!selected(name).includes(v)) toggleFilter(name, v)
      if (withGroup && !selected(groupName).includes(group)) toggleFilter(groupName, group)
    },
  })

  return (
    <div className="flex flex-col">
      <FilterPanels source="metrics" />

      <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
        <StatTile label="Model calls" value={formatCount(callCount)} loading={!calls.data} detail="in this window" />
        <StatTile
          label="Error rate"
          value={formatPercent(errorRate)}
          loading={!calls.data}
          tone={errorRate ? 'error' : undefined}
          detail={callTotals ? `${formatCount(callTotals.error ?? 0)} failed model calls` : undefined}
        />
        <StatTile
          label="Input tokens"
          value={formatCount(tokenTotals?.input)}
          loading={!tokens.data}
          detail={successCount ? `${formatCount((tokenTotals?.input ?? 0) / successCount)} per successful call` : undefined}
        />
        <StatTile
          label="Output tokens"
          value={formatCount(tokenTotals?.output)}
          loading={!tokens.data}
          detail={successCount ? `${formatCount((tokenTotals?.output ?? 0) / successCount)} per successful call` : undefined}
        />
        <StatTile
          label="Call duration p50"
          value={formatSeconds(q(durationBuckets, 0.5))}
          loading={!durationH.data}
          detail={`p95 ${formatSeconds(q(durationBuckets, 0.95))} · p99 ${formatSeconds(q(durationBuckets, 0.99))}`}
        />
        <StatTile
          label="Time to first chunk p50"
          value={formatSeconds(q(ttfcBuckets, 0.5))}
          loading={!ttfcH.data}
          detail={
            q(ttfcBuckets, 0.5) == null
              ? 'no streamed calls'
              : `p95 ${formatSeconds(q(ttfcBuckets, 0.95))} · p99 ${formatSeconds(q(ttfcBuckets, 0.99))}`
          }
        />
      </div>

      <SectionTitle>Trends</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Panel title="Model calls by outcome" subtitle={per(callsTrend.data)}>
          <TimeChart data={callsTrend.data} styles={OUTCOME_STYLES} stacked integer format={fmtCount} {...nav} />
        </Panel>
        <Panel title="Tokens" subtitle={per(tokensTrend.data)}>
          <TimeChart data={tokensTrend.data} styles={TOKEN_STYLES} format={fmtNumber} {...nav} />
        </Panel>
        <Panel title="Cache and reasoning tokens" subtitle={per(detailsTrend.data)}>
          <TimeChart data={detailsTrend.data} styles={DETAIL_STYLES} format={fmtNumber} {...nav} />
        </Panel>
      </div>

      <SectionTitle>Latency</SectionTitle>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Panel title="Call duration heatmap" subtitle="calls per time and duration" className="lg:col-span-2">
          <Heatmap
            data={durationHeat}
            height={180}
            formatBound={formatSeconds}
            onCell={listCalls}
            cellHint="Click to list the calls of about this time and duration"
          />
        </Panel>
        <Panel title="Call duration distribution" subtitle="calls per duration">
          <Distribution buckets={durationBuckets} />
        </Panel>
        <Panel title="Call duration" subtitle="p50, p95 and maximum">
          <TimeChart data={durationLines} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="Time to first chunk" subtitle="p50, p95 and maximum, streamed calls">
          <TimeChart data={ttfcLines} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="Time to first chunk heatmap" subtitle="streamed calls per time and wait">
          <Heatmap data={ttfcHeat} height={180} formatBound={formatSeconds} />
        </Panel>
      </div>

      <SectionTitle>Breakdown</SectionTitle>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <Panel title="By model" subtitle="tokens, input and output">
          <Ranking rows={rankRows(tokensByModel.data)} format={formatCount} empty="No tokens reported" {...rank('model', 'provider')} />
        </Panel>
        <Panel title="Failures by model" subtitle="calls that ended in an error">
          <Ranking rows={rankRows(failuresByModel.data)} format={formatCount} empty="No failed calls" {...rank('model', 'provider')} />
        </Panel>
        <Panel title="By service" subtitle="tokens, input and output">
          <Ranking rows={rankRows(tokensByService.data)} format={formatCount} empty="No tokens reported" {...rank('service')} />
        </Panel>
      </div>

      <ToolsSection {...nav} />
    </div>
  )
}

// The histogram chart over time as heatmap rows; a point's timestamp is the end of its interval.
function metricHeatmap(h: Series | undefined): HeatmapData | undefined {
  if (!h || !h.pointSeconds) return undefined
  const bounds = h.labels.map((le) => (le === '+Inf' ? null : Number(le)))
  return {
    times: h.times.map((t) => t - h.pointSeconds),
    step: h.pointSeconds,
    after: h.after,
    before: h.before,
    rows: bounds.map((upper, i) => ({ lower: i === 0 ? 0 : (bounds[i - 1] ?? 0), upper })),
    counts: h.values.map((row) => row.map((v) => v ?? 0)),
  }
}
