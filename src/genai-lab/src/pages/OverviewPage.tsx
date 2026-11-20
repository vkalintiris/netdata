import { useCallback, useMemo } from 'react'
import { Distribution } from '@/components/Distribution'
import { FilterBar } from '@/components/FilterBar'
import { Panel, SectionTitle } from '@/components/Panel'
import { Ranking, type RankRow } from '@/components/Ranking'
import { StatTile } from '@/components/StatTile'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { totals, type DataQuery, type Series } from '@/lib/agent'
import { formatCount, formatInterval, formatNumber, formatPercent, formatSeconds } from '@/lib/format'
import { absolute, LABELS, usePageState, type FilterName } from '@/lib/page-state'
import { quantile } from '@/lib/quantile'
import { useSeries } from '@/lib/queries'

const C = {
  calls: 'otel.derived_metrics.genai.calls',
  tokens: 'otel.derived_metrics.genai.tokens',
  details: 'otel.derived_metrics.genai.token_details',
  duration: 'otel.derived_metrics.genai.call_duration',
  ttfc: 'otel.derived_metrics.genai.time_to_first_chunk',
}

const POINTS = 120

const CALL_STYLES: SeriesStyle[] = [
  { label: 'success', name: 'Success', color: '--success' },
  { label: 'error', name: 'Error', color: '--error' },
]
const TOKEN_STYLES: SeriesStyle[] = [
  { label: 'input', name: 'Input', color: '--chart-1' },
  { label: 'output', name: 'Output', color: '--chart-2' },
]
const DETAIL_STYLES: SeriesStyle[] = [
  { label: 'cache_read', name: 'Cache read', color: '--chart-3' },
  { label: 'cache_write', name: 'Cache write', color: '--chart-4' },
  { label: 'reasoning', name: 'Reasoning', color: '--chart-5' },
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
  const { window, filters, setWindow, selected, toggleFilter } = usePageState()

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
  const durationAvg = useSeries(trend(`${C.duration}_avg`, { timeGroup: 'average', aggregation: 'average' }))
  const durationMax = useSeries(trend(`${C.duration}_max`, { timeGroup: 'max', aggregation: 'max' }))
  const ttfcAvg = useSeries(trend(`${C.ttfc}_avg`, { timeGroup: 'average', aggregation: 'average' }))
  const ttfcMax = useSeries(trend(`${C.ttfc}_max`, { timeGroup: 'max', aggregation: 'max' }))

  const by = (context: string, labels: string[]) => total(context, { groupByLabels: labels })
  const callsByModel = useSeries(by(C.calls, [LABELS.model, LABELS.provider]))
  const tokensByModel = useSeries(by(C.tokens, [LABELS.model, LABELS.provider]))
  const callsByProvider = useSeries(by(C.calls, [LABELS.provider]))
  const tokensByProvider = useSeries(by(C.tokens, [LABELS.provider]))
  const callsByService = useSeries(by(C.calls, [LABELS.service]))
  const tokensByService = useSeries(by(C.tokens, [LABELS.service]))

  const durationLines = useMemo(() => merge(durationAvg.data, durationMax.data), [durationAvg.data, durationMax.data])
  const ttfcLines = useMemo(() => merge(ttfcAvg.data, ttfcMax.data), [ttfcAvg.data, ttfcMax.data])

  const callTotals = calls.data ? totals(calls.data) : undefined
  const tokenTotals = tokens.data ? totals(tokens.data) : undefined
  const durationBuckets = durationH.data ? totals(durationH.data) : undefined
  const ttfcBuckets = ttfcH.data ? totals(ttfcH.data) : undefined

  const callCount = callTotals ? (callTotals.success ?? 0) + (callTotals.error ?? 0) : undefined
  const errorRate = callTotals && callCount ? (callTotals.error ?? 0) / callCount : callCount === 0 ? 0 : undefined
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
  const onReset = useCallback(() => setWindow({ after: -3600, before: 0 }), [setWindow])
  const nav = { onZoom, onPan, onReset }

  const per = (s: Series | undefined) => (s?.pointSeconds ? `per ${formatInterval(s.pointSeconds)}` : undefined)
  const rank = (name: FilterName) => ({ selected: selected(name), onSelect: (v: string) => toggleFilter(name, v) })

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
          detail={callCount ? `${formatCount((tokenTotals?.input ?? 0) / callCount)} per call` : undefined}
        />
        <StatTile
          label="Output tokens"
          value={formatCount(tokenTotals?.output)}
          loading={!tokens.data}
          detail={callCount ? `${formatCount((tokenTotals?.output ?? 0) / callCount)} per call` : undefined}
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
          <TimeChart data={callsTrend.data} styles={CALL_STYLES} stacked format={fmtCount} {...nav} />
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
        <Panel title="Call duration" subtitle="average and maximum">
          <TimeChart data={durationLines} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="Call duration distribution" subtitle="calls in this window">
          <Distribution buckets={durationBuckets} />
        </Panel>
        <Panel title="Time to first chunk" subtitle="average and maximum, streamed calls">
          <TimeChart data={ttfcLines} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
      </div>

      <SectionTitle>Breakdown</SectionTitle>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <Panel title="By model" subtitle="calls">
          <Ranking rows={rankRows(callsByModel.data)} format={formatCount} empty="No calls" {...rank('model')} />
        </Panel>
        <Panel title="By provider" subtitle="calls">
          <Ranking rows={rankRows(callsByProvider.data)} format={formatCount} empty="No calls" {...rank('provider')} />
        </Panel>
        <Panel title="By service" subtitle="calls">
          <Ranking rows={rankRows(callsByService.data)} format={formatCount} empty="No calls" {...rank('service')} />
        </Panel>
        <Panel title="By model" subtitle="tokens, input and output">
          <Ranking rows={rankRows(tokensByModel.data)} format={formatCount} empty="No tokens reported" {...rank('model')} />
        </Panel>
        <Panel title="By provider" subtitle="tokens, input and output">
          <Ranking rows={rankRows(tokensByProvider.data)} format={formatCount} empty="No tokens reported" {...rank('provider')} />
        </Panel>
        <Panel title="By service" subtitle="tokens, input and output">
          <Ranking rows={rankRows(tokensByService.data)} format={formatCount} empty="No tokens reported" {...rank('service')} />
        </Panel>
      </div>
    </div>
  )
}

const LATENCY_STYLES: SeriesStyle[] = [
  { label: 'average', name: 'Average', color: '--chart-1' },
  { label: 'maximum', name: 'Maximum', color: '--chart-5' },
]

// The average and maximum charts are separate contexts with one dimension each; draw them on one chart.
function merge(avg: Series | undefined, max: Series | undefined): Series | undefined {
  if (!avg || !max) return undefined
  const byTime = new Map(max.times.map((t, i) => [t, max.values[0]?.[i] ?? null]))
  return {
    ...avg,
    labels: ['average', 'maximum'],
    values: [avg.values[0] ?? avg.times.map(() => null), avg.times.map((t) => byTime.get(t) ?? null)],
  }
}
