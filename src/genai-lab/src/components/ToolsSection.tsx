import { useMemo } from 'react'
import { Panel, SectionTitle } from '@/components/Panel'
import { TimeChart } from '@/components/TimeChart'
import { Skeleton } from '@/components/ui/skeleton'
import type { DataQuery, Filters } from '@/lib/agent'
import { LATENCY_STYLES, latencyLines, OUTCOME_STYLES } from '@/lib/charts'
import { formatCount, formatInterval, formatSeconds } from '@/lib/format'
import { LABELS, POINTS, usePageState } from '@/lib/page-state'
import { useSeries } from '@/lib/queries'
import { TOOL_LABEL, toolRows, UNNAMED_TOOL, type ToolRow } from '@/lib/tools'

const C = {
  calls: 'otel.derived_metrics.genai.tool_calls',
  duration: 'otel.derived_metrics.genai.tool_call_duration',
}

const LIMIT = 8

const fmtCount = (v: number | null) => formatCount(v)
const fmtSeconds = (v: number | null) => formatSeconds(v)

function ToolTable({ rows }: { rows: ToolRow[] }) {
  if (!rows.length) return <p className="py-6 text-center text-sm text-muted-foreground">No tool calls</p>
  const top = rows.slice(0, LIMIT)
  const rest = rows.slice(LIMIT)
  const max = top[0].calls
  return (
    <table className="w-full text-sm">
      <thead>
        <tr className="text-xs text-muted-foreground">
          <th className="w-full px-2 py-1 text-left font-normal">Tool</th>
          <th className="px-3 py-1 text-right font-normal">Calls</th>
          <th className="px-3 py-1 text-right font-normal">Failed</th>
          <th className="px-2 py-1 text-right font-normal" title="The median duration of a call">
            Typical
          </th>
          <th className="px-2 py-1 text-right font-normal" title="The longest call">
            Slowest
          </th>
        </tr>
      </thead>
      <tbody>
        {top.map((r) => (
          <tr key={r.name}>
            <td className="relative max-w-0 px-2 py-1">
              <span className="absolute inset-y-0.5 left-0 rounded bg-primary/12" style={{ width: `${(r.calls / max) * 100}%` }} />
              {r.name === UNNAMED_TOOL ? (
                <span className="relative block truncate text-muted-foreground" title="The producer does not name its tools">
                  (no name)
                </span>
              ) : (
                <span className="relative block truncate" title={r.name}>
                  {r.name}
                </span>
              )}
            </td>
            <td className="px-3 py-1 text-right font-mono text-xs tabular-nums">{formatCount(r.calls)}</td>
            <td
              className={
                'px-3 py-1 text-right font-mono text-xs tabular-nums ' +
                (r.failed > 0 ? 'text-[var(--error)]' : 'text-muted-foreground')
              }
            >
              {formatCount(r.failed)}
            </td>
            <td className="px-2 py-1 text-right font-mono text-xs whitespace-nowrap tabular-nums">
              {r.typical == null ? '–' : `${r.typicalIsLimit ? '≤ ' : ''}${formatSeconds(r.typical)}`}
            </td>
            <td className="px-2 py-1 text-right font-mono text-xs whitespace-nowrap tabular-nums">{formatSeconds(r.slowest)}</td>
          </tr>
        ))}
        {rest.length > 0 && (
          <tr className="text-xs text-muted-foreground">
            <td className="px-2 py-1">{rest.length} more</td>
            <td className="px-2 py-1 text-right font-mono tabular-nums">{formatCount(rest.reduce((s, r) => s + r.calls, 0))}</td>
            <td className="px-2 py-1 text-right font-mono tabular-nums">{formatCount(rest.reduce((s, r) => s + r.failed, 0))}</td>
            <td colSpan={2} />
          </tr>
        )}
      </tbody>
    </table>
  )
}

// Tool runs reported by agent frameworks. A tool span carries no model or provider, so only the Service filter
// reaches these charts.
export function ToolsSection(nav: {
  onZoom: (after: number, before: number) => void
  onPan: (seconds: number) => void
  onReset: () => void
}) {
  const { window, filters } = usePageState()
  const service = filters[LABELS.service]
  const base = useMemo(() => {
    const scoped: Filters = service ? { [LABELS.service]: service } : {}
    return { window, filters: scoped }
  }, [window, service])
  const query = (context: string, points: number, extra: Partial<DataQuery> = {}): DataQuery => ({
    ...base,
    context,
    points,
    timeGroup: 'sum',
    aggregation: 'sum',
    ...extra,
  })
  const maximum = { timeGroup: 'max', aggregation: 'max' } as const
  const byTool = { groupByLabels: [TOOL_LABEL] }

  const callsTrend = useSeries(query(C.calls, POINTS))
  const histogramTrend = useSeries(query(`${C.duration}_histogram`, POINTS))
  const maxTrend = useSeries(query(`${C.duration}_max`, POINTS, maximum))
  const callsByTool = useSeries(query(C.calls, 1, { ...byTool, groupByDimension: true }))
  const histogramByTool = useSeries(query(`${C.duration}_histogram`, 1, { ...byTool, groupByDimension: true }))
  const maxByTool = useSeries(query(`${C.duration}_max`, 1, { ...byTool, ...maximum }))

  const lines = useMemo(() => latencyLines(histogramTrend.data, maxTrend.data), [histogramTrend.data, maxTrend.data])
  const rows = useMemo(
    () => toolRows(callsByTool.data, histogramByTool.data, maxByTool.data),
    [callsByTool.data, histogramByTool.data, maxByTool.data],
  )
  const unapplied = Object.keys(filters).some((key) => key !== LABELS.service)
  const per = callsTrend.data?.pointSeconds ? `per ${formatInterval(callsTrend.data.pointSeconds)}` : undefined

  return (
    <>
      <SectionTitle>Tools</SectionTitle>
      {unapplied && (
        <p className="mb-2 text-xs text-muted-foreground">
          The Model and Provider filters do not apply to tools: a tool call carries neither.
        </p>
      )}
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-4">
        <Panel title="Tool calls by outcome" subtitle={per}>
          <TimeChart data={callsTrend.data} styles={OUTCOME_STYLES} stacked integer format={fmtCount} {...nav} />
        </Panel>
        <Panel title="Tool call duration" subtitle="p50, p95 and maximum">
          <TimeChart data={lines} styles={LATENCY_STYLES} format={fmtSeconds} {...nav} />
        </Panel>
        <Panel title="By tool" subtitle="calls, failures and duration" className="lg:col-span-2">
          {callsByTool.data ? <ToolTable rows={rows} /> : <Skeleton className="h-24 w-full" />}
        </Panel>
      </div>
    </>
  )
}
