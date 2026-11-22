import { useCallback, useMemo } from 'react'
import { useNavigate } from 'react-router'
import { Distribution } from '@/components/Distribution'
import { Heatmap, type HeatCell, type HeatmapData } from '@/components/Heatmap'
import { cellParam } from '@/lib/traces'
import { FilterBar } from '@/components/FilterBar'
import { Panel, SectionTitle } from '@/components/Panel'
import { Ranking, type RankRow } from '@/components/Ranking'
import { StatTile } from '@/components/StatTile'
import { ToolsSection } from '@/components/ToolsSection'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { totals, type DataQuery, type Series } from '@/lib/agent'
import { LATENCY_STYLES, latencyLines, OUTCOME_STYLES } from '@/lib/charts'
import { formatCount, formatInterval, formatNumber, formatPercent, formatSeconds } from '@/lib/format'
import { absolute, LABELS, POINTS, resetWindow, usePageState, type FilterName } from '@/lib/page-state'
import { quantile } from '@/lib/quantile'
import { useSeries } from '@/lib/queries'

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

export function OverviewPage() {
  const { window, filters, setWindow, selected, toggleFilter, search } = usePageState()
  const navigate = useNavigate()

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
  const callsByModel = useSeries(by(C.calls, [LABELS.model, LABELS.provider]))
  const tokensByModel = useSeries(by(C.tokens, [LABELS.model, LABELS.provider]))
  const callsByProvider = useSeries(by(C.calls, [LABELS.provider]))
  const failuresByModel = useSeries(total(C.calls, { groupByLabels: [LABELS.model, LABELS.provider], dimensions: ['error'] }))
  const callsByService = useSeries(by(C.calls, [LABELS.service]))
  const tokensByService = useSeries(by(C.tokens, [LABELS.service]))

  const durationLines = useMemo(() => latencyLines(durationTrendH.data, durationMax.data), [durationTrendH.data, durationMax.data])
  const durationHeat = useMemo(() => metricHeatmap(durationTrendH.data), [durationTrendH.data])
  const ttfcHeat = useMemo(() => metricHeatmap(ttfcTrendH.data), [ttfcTrendH.data])
  // A duration cell opens the Calls page on the same window, listing the spans of that cell.
  const openCell = (c: HeatCell, d: HeatmapData) => {
    const p = new URLSearchParams(search)
    const row = d.rows[c.row]
    p.set('cell', cellParam({ after: d.times[c.col], before: d.times[c.col] + d.step, min: row.lower, max: row.upper }))
    navigate({ pathname: '/calls', search: `?${p}` })
  }
  const ttfcLines = useMemo(() => latencyLines(ttfcTrendH.data, ttfcMax.data), [ttfcTrendH.data, ttfcMax.data])

  const callTotals = calls.data ? totals(calls.data) : undefined
  const tokenTotals = tokens.data ? totals(tokens.data) : undefined
  const durationBuckets = durationH.data ? totals(durationH.data) : undefined
  const ttfcBuckets = ttfcH.data ? totals(ttfcH.data) : undefined

  const callCount = callTotals ? (callTotals.success ?? 0) + (callTotals.error ?? 0) : undefined
  const errorRate = callTotals && callCount ? (callTotals.error ?? 0) / callCount : undefined
  const successCount = callTotals?.success ?? 0
  const q = (b: Record<string, number> | undefined, p: number) => (b ? quantile(p, b) : null)

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
  const onReset = useCallback(() => setWindow(resetWindow()), [setWindow])
  const nav = { onZoom, onPan, onReset }

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
      <FilterBar />

      <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
        <StatTile label="Calls" value={formatCount(callCount)} loading={!calls.data} detail="in this window" />
        <StatTile
          label="Error rate"
          value={formatPercent(errorRate)}
          loading={!calls.data}
          tone={errorRate ? 'error' : undefined}
          detail={callTotals ? `${formatCount(callTotals.error ?? 0)} failed calls` : undefined}
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
        <Panel title="Calls by outcome" subtitle={per(callsTrend.data)}>
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
        <Panel title="Call duration heatmap" subtitle="calls per time and duration; click a cell to list its calls" className="lg:col-span-2">
          <Heatmap data={durationHeat} height={180} formatBound={formatSeconds} onCell={openCell} cellHint="Click to list these calls" />
        </Panel>
        <Panel title="Call duration distribution" subtitle="calls with a recorded duration">
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
        <Panel title="By model" subtitle="calls">
          <Ranking rows={rankRows(callsByModel.data)} format={formatCount} empty="No calls" {...rank('model', 'provider')} />
        </Panel>
        <Panel title="By provider" subtitle="calls">
          <Ranking rows={rankRows(callsByProvider.data)} format={formatCount} empty="No calls" {...rank('provider')} />
        </Panel>
        <Panel title="By service" subtitle="calls">
          <Ranking rows={rankRows(callsByService.data)} format={formatCount} empty="No calls" {...rank('service')} />
        </Panel>
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
