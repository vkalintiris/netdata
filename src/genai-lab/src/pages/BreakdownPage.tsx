import { useQuery } from '@tanstack/react-query'
import { Check, ChevronDown, Info } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router'
import { FilterPanels, SpanFilterBar } from '@/components/FilterPanels'
import { NUM } from '@/components/ListFrame'
import { Panel } from '@/components/Panel'
import { TimeChart, type SeriesStyle } from '@/components/TimeChart'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import {
  breakdownRows,
  breakdownSeries,
  isMeasure,
  measureOf,
  MEASURES,
  noneOf,
  ranked,
  SUM_KEYS,
  type BreakdownRow,
  type Measure,
} from '@/lib/breakdown'
import { formatCount, formatInterval, formatPercent, formatSeconds } from '@/lib/format'
import { LABELS, unitOf, useChartNav, usePageState, type FilterName, type SpanQuery } from '@/lib/page-state'
import { exploreGroups, groupableKeys, K, SPAN_KEY_OF } from '@/lib/traces'
import { cn } from '@/lib/utils'

// Keys worth offering first, by the name a person would look for.
const SUGGESTED: { key: string; name: string }[] = [
  { key: K.service, name: 'Service' },
  { key: K.requestModel, name: 'Model' },
  { key: K.responseModel, name: 'Answering model' },
  { key: K.provider, name: 'Provider' },
  { key: K.operation, name: 'Operation' },
]

const DEFAULT_KEY = K.service

// Values drawn on the trend; the rest add up as one series.
const TREND_VALUES = 7
const REST = 'other values'
const COLORS = ['--chart-1', '--chart-2', '--chart-3', '--chart-4', '--chart-5', '--chart-6', '--chart-7']

// A stored key as a person names it: where it lives is a tag, not part of the name.
function keyName(key: string): { name: string; scope: string | null } {
  const suggested = SUGGESTED.find((s) => s.key === key)
  if (suggested) return { name: suggested.name, scope: null }
  if (key.startsWith('resource.attributes.')) return { name: key.slice('resource.attributes.'.length), scope: 'resource' }
  if (key.startsWith('attributes.')) return { name: key.slice('attributes.'.length), scope: null }
  return { name: key, scope: 'span' }
}

function KeyPicker({ value, onChange }: { value: string; onChange: (key: string) => void }) {
  const { window } = usePageState()
  const [open, setOpen] = useState(false)
  const keys = useQuery({
    queryKey: ['groupable-keys', window],
    queryFn: ({ signal }) => groupableKeys(window, signal),
    enabled: open,
    staleTime: 60_000,
  })
  const others = useMemo(() => {
    const pinned = new Set(SUGGESTED.map((s) => s.key))
    return (keys.data ?? []).filter((k) => !pinned.has(k.text)).sort((a, b) => a.text.localeCompare(b.text))
  }, [keys.data])
  const current = keyName(value)
  const pick = (key: string) => {
    onChange(key)
    setOpen(false)
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1.5">
          <span className="text-muted-foreground">Group by</span>
          <span className="font-medium">{current.name}</span>
          {current.scope && <span className="text-xs text-muted-foreground">{current.scope}</span>}
          <ChevronDown className="size-3.5 text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-96 p-0">
        <Command>
          <CommandInput placeholder="Find an attribute" />
          <CommandList className="max-h-80">
            <CommandEmpty>{keys.isLoading ? 'Loading…' : 'No attribute matches.'}</CommandEmpty>
            <CommandGroup heading="Common">
              {SUGGESTED.map((s) => (
                <CommandItem key={s.key} value={`${s.name} ${s.key}`} onSelect={() => pick(s.key)}>
                  <Check className={cn('size-3.5', s.key !== value && 'invisible')} />
                  {s.name}
                </CommandItem>
              ))}
            </CommandGroup>
            <CommandGroup heading="Every attribute in this window">
              {keys.isLoading && <Skeleton className="mx-2 my-1 h-5" />}
              {others.map((k) => {
                const n = keyName(k.text)
                return (
                  <CommandItem key={k.text} value={k.text} onSelect={() => pick(k.text)}>
                    <Check className={cn('size-3.5', k.text !== value && 'invisible')} />
                    <span className="truncate">{n.name}</span>
                    {n.scope && <span className="ml-auto text-xs text-muted-foreground">{n.scope}</span>}
                  </CommandItem>
                )
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

function formatMeasure(measure: Measure, v: number | null | undefined): string {
  if (v == null) return '–'
  return measure === 'time' ? formatSeconds(v) : formatCount(v)
}

// Model calls added up per value of an attribute the user picks: the question the fixed breakdowns of the Metrics
// tab cannot be asked.
export function BreakdownPage() {
  const { window, spanQuery: query } = usePageState()
  const [params, setParams] = useSearchParams()
  const navigate = useNavigate()
  const nav = useChartNav()
  const key = params.get('by') ?? DEFAULT_KEY
  const measure: Measure = isMeasure(params.get('measure')) ? (params.get('measure') as Measure) : 'calls'

  // The failed among the calls the page shows: none when the Outcome filter keeps only the succeeded.
  const showsFailed = query.outcomes.length === 0 || query.outcomes.includes('failed')
  const failedQuery: SpanQuery = useMemo(() => ({ ...query, outcomes: ['failed'] }), [query])
  const live = window.after < 0
  const all = useQuery({
    queryKey: ['breakdown', window, query, key],
    queryFn: ({ signal }) => exploreGroups(window, query, key, SUM_KEYS, signal),
    refetchInterval: live ? 15_000 : false,
    placeholderData: (prev) => prev,
  })
  const failedAnswer = useQuery({
    queryKey: ['breakdown-failed', window, failedQuery, key],
    queryFn: ({ signal }) => exploreGroups(window, failedQuery, key, [], signal),
    enabled: showsFailed,
    refetchInterval: live ? 15_000 : false,
    placeholderData: (prev) => prev,
  })
  const failures = useMemo(
    () => (showsFailed ? failedAnswer.data : noneOf(all.data)),
    [showsFailed, failedAnswer.data, all.data],
  )

  const rows = useMemo(() => ranked(breakdownRows(all.data, failures), measure), [all.data, failures, measure])
  // Shares are of the calls that carry the attribute.
  const total = rows.reduce((s, r) => s + (r.kind === 'unset' ? 0 : (measureOf(r, measure) ?? 0)), 0)
  // The bars compare the named values; what could not be named is often far larger and would flatten them.
  const top = Math.max(1, ...rows.filter((r) => r.kind === 'value').map((r) => measureOf(r, measure) ?? 0))
  const shown = useMemo(
    () =>
      rows
        .filter((r) => r.kind === 'value' && (measureOf(r, measure) ?? 0) > 0)
        .slice(0, TREND_VALUES)
        .map((r) => r.value),
    [rows, measure],
  )
  const series = useMemo(
    () => breakdownSeries(all.data, failures, measure, shown, REST),
    [all.data, failures, measure, shown],
  )
  const styles: SeriesStyle[] = [
    ...shown.map((value, i) => ({ label: value, color: COLORS[i % COLORS.length] })),
    { label: REST, color: '--chart-axis' },
  ]
  const unsetCalls = rows.find((r) => r.kind === 'unset')?.calls ?? 0
  const skipped = all.data?.histogram?.sums?.reduce((s, one) => s + one.skipped, 0) ?? 0

  const set = (name: string, value: string | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      if (value) next.set(name, value)
      else next.delete(name)
      return next
    })

  // The traces behind a row, on the Traces tab: the page's own filter for a key that has one, a condition otherwise.
  const open = (row: BreakdownRow) => {
    const next = new URLSearchParams(params)
    next.delete('by')
    next.delete('measure')
    const name = (Object.keys(LABELS) as FilterName[]).find((n) => SPAN_KEY_OF[LABELS[n]] === key)
    if (name) {
      if (!next.getAll(name).includes(row.value)) next.append(name, row.value)
    } else if (!next.getAll('where').includes(`${key}=${row.value}`)) {
      next.append('where', `${key}=${row.value}`)
    }
    navigate(`/traces?${next}`)
  }

  const named = keyName(key)
  return (
    <div className="flex flex-col gap-3">
      <FilterPanels source="spans" />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <KeyPicker value={key} onChange={(k) => set('by', k === DEFAULT_KEY ? null : k)} />
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={measure}
          onValueChange={(v) => v && set('measure', v === 'calls' ? null : v)}
        >
          {(Object.keys(MEASURES) as Measure[]).map((m) => (
            <ToggleGroupItem key={m} value={m}>
              {MEASURES[m]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <SpanFilterBar />
      </div>

      {all.error ? (
        <p className="rounded-lg border p-6 text-sm text-destructive">{String((all.error as Error).message)}</p>
      ) : !all.data ? (
        <Skeleton className="h-96 w-full" />
      ) : (
        <>
          <Panel
            title={`${MEASURES[measure]} by ${named.name}`}
            subtitle={
              series?.pointSeconds
                ? `per ${formatInterval(series.pointSeconds)}${unsetCalls > 0 ? `; ${formatCount(unsetCalls)} calls without the attribute are not drawn` : ''}`
                : undefined
            }
          >
            <TimeChart
              data={series}
              styles={styles}
              stacked
              integer={measure !== 'time'}
              format={(v) => formatMeasure(measure, v)}
              height={220}
              {...nav}
            />
          </Panel>

          <div className="min-w-0 rounded-lg border bg-card">
            {rows.length === 0 ? (
              <p className="p-6 text-center text-sm text-muted-foreground">Nothing matches in this window.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{named.name}</TableHead>
                    <TableHead className="w-[22%]">Share of {MEASURES[measure].toLowerCase()}</TableHead>
                    <TableHead className="text-right">Calls</TableHead>
                    <TableHead className="text-right">Failed</TableHead>
                    <TableHead className="text-right">Tokens in</TableHead>
                    <TableHead className="text-right">Tokens out</TableHead>
                    <TableHead className="text-right">Time in calls</TableHead>
                    <TableHead className="text-right">Average call</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => {
                    const v = measureOf(row, measure)
                    const share = total > 0 && v != null && row.kind !== 'unset' ? v / total : null
                    const clickable = row.kind === 'value'
                    return (
                      <TableRow
                        key={`${row.kind}:${row.value}`}
                        onClick={clickable ? () => open(row) : undefined}
                        className={clickable ? 'cursor-pointer' : 'text-muted-foreground'}
                        title={
                          row.kind === 'other'
                            ? 'Values past the 20 most frequent, added up'
                            : row.kind === 'unset'
                              ? 'Calls whose span lacks this attribute'
                              : 'List the traces of these calls'
                        }
                      >
                        <TableCell className={cn('max-w-80 truncate', clickable && 'font-medium')}>{row.value}</TableCell>
                        <TableCell>
                          <div className="flex items-center gap-2">
                            <div className="h-2 min-w-0 flex-1 rounded bg-muted">
                              <div
                                className={cn('h-2 rounded', clickable ? 'bg-primary/60' : 'bg-muted-foreground/30')}
                                style={{ width: `${v != null ? Math.min(100, Math.max(0, (v / top) * 100)) : 0}%` }}
                              />
                            </div>
                            <span className="w-12 text-right font-mono text-xs tabular-nums">{formatPercent(share)}</span>
                          </div>
                        </TableCell>
                        <TableCell className={NUM}>{formatCount(row.calls)}</TableCell>
                        <TableCell className={cn(NUM, row.failed > 0 && 'text-[var(--error)]')}>
                          {row.failed > 0 ? formatCount(row.failed) : '–'}
                        </TableCell>
                        <TableCell className={NUM}>{formatCount(row.tokensIn)}</TableCell>
                        <TableCell className={NUM}>{formatCount(row.tokensOut)}</TableCell>
                        <TableCell className={NUM}>{formatSeconds(row.seconds)}</TableCell>
                        <TableCell className={NUM}>
                          {formatSeconds(row.seconds != null && row.calls > 0 ? row.seconds / row.calls : null)}
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
            )}
            <p className="flex items-start gap-2 border-t px-4 py-2 text-xs text-muted-foreground">
              <Info className="mt-px size-3.5 shrink-0" />
              <span>
                Added up from the stored spans of {unitOf(query.kinds)}, each once. The attribute must be on the call's own
                span or on its resource. At most 20 values are named, the most frequent ones; an attribute with a
                thousand or more values in one stored file cannot be grouped and shows as other values.
                {skipped > 0 && ` ${formatCount(skipped)} values were not whole numbers and were left out.`}
              </span>
            </p>
          </div>
        </>
      )}
    </div>
  )
}
