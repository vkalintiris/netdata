import { useQuery } from '@tanstack/react-query'
import { ArrowLeft, Info, Wrench } from 'lucide-react'
import { useMemo } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import { MessageView, SpanDetail, Waterfall } from '@/components/SpanDetail'
import { StatTile } from '@/components/StatTile'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { formatCount, formatSeconds, formatUsd } from '@/lib/format'
import { GATEWAY_PAGES } from '@/lib/gateways'
import { fetchTrace, field, isUnpriced, K, type TraceSpan, usdOf } from '@/lib/traces'
import { transcriptOf, type CallStep, type Step, type ToolStep } from '@/lib/transcript'
import { cn } from '@/lib/utils'

const dateFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

function tokens(span: TraceSpan, key: string): number | null {
  const v = field(span.fields, key)
  return v == null ? null : Number(v)
}

function sum(steps: CallStep[], key: string): number | null {
  let total: number | null = null
  for (const step of steps) {
    const v = tokens(step.span, key)
    if (v != null && !Number.isNaN(v)) total = (total ?? 0) + v
  }
  return total
}

function failed(span: TraceSpan): boolean {
  return field(span.fields, K.outcome) === 'error'
}

function CallStepView({
  step,
  index,
  active,
  onOpen,
}: {
  step: CallStep
  index: number
  active: boolean
  onOpen: () => void
}) {
  const f = (key: string) => field(step.span.fields, key)
  const tokensIn = tokens(step.span, K.inputTokens)
  const tokensOut = tokens(step.span, K.outputTokens)
  const usd = usdOf(step.span.fields)
  return (
    <li className={cn('rounded-lg border bg-card', active && 'border-primary/60')}>
      <button
        type="button"
        onClick={onOpen}
        title="Open this call's details"
        className="flex w-full flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-2 text-left text-xs hover:bg-muted/50"
      >
        <span className="font-mono text-muted-foreground tabular-nums">{index}</span>
        <span className="text-sm font-medium">{f(K.requestModel) ?? 'Model call'}</span>
        <span className="text-muted-foreground">{f(K.provider)}</span>
        {failed(step.span) && (
          <Badge variant="destructive" title={f(K.errorType)}>
            error
          </Badge>
        )}
        <span className="ml-auto flex gap-3 font-mono tabular-nums text-muted-foreground">
          <span>
            {tokensIn == null ? '–' : formatCount(tokensIn)} in · {tokensOut == null ? '–' : formatCount(tokensOut)} out
          </span>
          {usd != null && <span title="Estimated from the tokens and published prices">{formatUsd(usd)}</span>}
          {isUnpriced(step.span.fields) && <span title="This model has no known price">no price</span>}
          <span>{formatSeconds(step.span.duration_ns / 1e9)}</span>
        </span>
      </button>
      <div className="flex flex-col gap-4 p-3">
        {step.startsOver && (
          <p className="text-xs text-muted-foreground">
            This call continues none of the calls above: another conversation of the same trace.
          </p>
        )}
        {step.notices.map((n) => (
          <p key={n} className="text-xs text-[var(--error)]">
            {n}
          </p>
        ))}
        {!step.hasContent ? (
          <p className="text-sm text-muted-foreground">
            No messages on this call. Most instrumentations record them only when content capture is turned on.
          </p>
        ) : (
          <>
            {step.added.map((m, i) => (
              <MessageView key={`in-${i}`} message={m} />
            ))}
            {step.answer.map((m, i) => (
              <MessageView key={`out-${i}`} message={m} />
            ))}
          </>
        )}
      </div>
    </li>
  )
}

function ToolStepView({ step, active, onOpen }: { step: ToolStep; active: boolean; onOpen: () => void }) {
  const name = field(step.span.fields, K.tool, K.name) ?? 'tool'
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        title="Open this tool call's details"
        className={cn(
          'ml-6 flex w-[calc(100%-1.5rem)] items-center gap-2 rounded-md border border-dashed px-3 py-1.5 text-left text-xs hover:bg-muted/50',
          active && 'border-primary/60',
        )}
      >
        <Wrench className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate font-medium">{name}</span>
        {failed(step.span) && <Badge variant="destructive">error</Badge>}
        <span className="ml-auto font-mono tabular-nums text-muted-foreground">
          {formatSeconds(step.span.duration_ns / 1e9)}
        </span>
      </button>
    </li>
  )
}

export function TracePage() {
  const { traceId = '' } = useParams()
  const [params, setParams] = useSearchParams()
  const at = Number(params.get('at'))
  const selected = params.get('span')

  const hasTime = traceId !== '' && Number.isFinite(at) && at > 0
  const trace = useQuery({
    queryKey: ['trace', traceId, String(at)],
    queryFn: ({ signal }) => fetchTrace(traceId, at, signal),
    enabled: hasTime,
  })

  const spans = trace.data?.spans
  const steps: Step[] = useMemo(() => (spans ? transcriptOf(spans) : []), [spans])
  const calls = steps.filter((s): s is CallStep => s.kind === 'call')
  const tools = steps.filter((s): s is ToolStep => s.kind === 'tool')

  const select = (span: TraceSpan | null) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        const id = span ? `${traceId}:${span.span_id}:${span.start_ns}` : null
        if (id && id !== selected) next.set('span', id)
        else next.delete('span')
        return next
      },
      { replace: true },
    )

  // Back to the list with its window and filters; the open span and the trace's own time stay behind. A trace opened
  // from a gateway's page goes back there.
  const back = new URLSearchParams(params)
  back.delete('span')
  back.delete('at')
  const gateway = GATEWAY_PAGES[back.get('from') ?? '']
  back.delete('from')

  const start = spans?.length ? Math.min(...spans.map((s) => s.start_ns)) : null
  const end = spans?.length ? Math.max(...spans.map((s) => s.start_ns + s.duration_ns)) : null
  const first = steps[0]?.span
  const models = [...new Set(calls.map((c) => field(c.span.fields, K.requestModel)).filter(Boolean))]
  const failedCalls = calls.filter((c) => failed(c.span)).length
  const failedTools = tools.filter((t) => failed(t.span)).length
  const tokensIn = sum(calls, K.inputTokens)
  const tokensOut = sum(calls, K.outputTokens)
  const priced = calls.map((c) => usdOf(c.span.fields)).filter((usd) => usd != null)
  const usd = priced.length ? priced.reduce((s, v) => s + v, 0) : null
  const unpriced = calls.filter((c) => isUnpriced(c.span.fields)).length
  const withContent = calls.filter((c) => c.hasContent).length
  const selectedSpanId = selected?.split(':')[1] ?? ''
  let callIndex = 0

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <Link
          to={{ pathname: gateway?.path ?? '/traces', search: back.toString() ? `?${back}` : '' }}
          className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="size-4" />
          {gateway?.label ?? 'Traces'}
        </Link>
        <h2 className="text-lg font-semibold">{first ? (field(first.fields, K.service) ?? 'Trace') : 'Trace'}</h2>
        {start != null && <span className="text-sm text-muted-foreground">{dateFmt.format(start / 1e6)}</span>}
        <span className="font-mono text-xs text-muted-foreground">{traceId}</span>
      </div>

      {trace.error ? (
        <p className="rounded-lg border p-6 text-sm text-destructive">{String((trace.error as Error).message)}</p>
      ) : !hasTime ? (
        <p className="rounded-lg border p-6 text-sm text-muted-foreground">
          This link lacks the trace's time. Open the trace from the Traces list.
        </p>
      ) : !spans ? (
        <Skeleton className="h-64 w-full" />
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-7">
            <StatTile
              label="Model calls"
              value={formatCount(calls.length)}
              detail={failedCalls ? `${formatCount(failedCalls)} failed` : models.join(', ')}
              tone={failedCalls ? 'error' : undefined}
            />
            <StatTile
              label="Tool calls"
              value={formatCount(tools.length)}
              detail={failedTools ? `${formatCount(failedTools)} failed` : undefined}
              tone={failedTools ? 'error' : undefined}
            />
            <StatTile label="Tokens in" value={formatCount(tokensIn)} detail="added up over the calls" />
            <StatTile label="Tokens out" value={formatCount(tokensOut)} />
            <StatTile
              label="Estimated cost"
              value={formatUsd(usd)}
              detail={
                unpriced > 0
                  ? `${formatCount(unpriced)} calls of a model without a known price left out`
                  : 'tokens at published prices'
              }
            />
            <StatTile
              label="Duration"
              value={start != null && end != null ? formatSeconds((end - start) / 1e9) : '–'}
              detail="first span's start to last span's end"
            />
            <StatTile
              label="Time in model calls"
              value={formatSeconds(calls.reduce((t, c) => t + c.span.duration_ns, 0) / 1e9)}
              detail="added up; parallel calls overlap"
            />
          </div>

          {trace.data?.cut && (
            <p className="flex items-start gap-2 rounded-lg border border-[var(--error)]/50 px-3 py-2 text-sm">
              <Info className="mt-0.5 size-4 shrink-0" />
              <span>
                This trace is past the size limit of one answer: the Agent sent its earliest {formatCount(spans.length)}{' '}
                spans only. The numbers above and the steps below cover that part, up to{' '}
                {dateFmt.format(Math.max(...spans.map((s) => s.start_ns)) / 1e6)}; the Traces list counts the whole trace.
              </span>
            </p>
          )}

          <div className={cn('grid gap-4', selected && 'xl:grid-cols-[minmax(0,1fr)_560px]')}>
            <Tabs defaultValue="conversation" className="min-w-0 gap-3">
              <TabsList>
                <TabsTrigger value="conversation">Conversation</TabsTrigger>
                <TabsTrigger value="spans">All spans ({formatCount(spans.length)})</TabsTrigger>
              </TabsList>

              <TabsContent value="conversation" className="flex flex-col gap-3">
                {steps.length === 0 ? (
                  <p className="rounded-lg border p-6 text-sm text-muted-foreground">
                    This trace has no model call or tool call the plugin recognised.
                  </p>
                ) : (
                  <>
                    {calls.length > 0 && withContent < calls.length && (
                      <p className="flex items-start gap-2 text-xs text-muted-foreground">
                        <Info className="mt-px size-3.5 shrink-0" />
                        {withContent === 0
                          ? 'No message content was sent for this trace, so the steps show without their text.'
                          : `${formatCount(calls.length - withContent)} of ${formatCount(calls.length)} calls carry no message content.`}
                      </p>
                    )}
                    <ol className="flex flex-col gap-2">
                      {steps.map((step) =>
                        step.kind === 'call' ? (
                          <CallStepView
                            key={step.span.span_id}
                            step={step}
                            index={++callIndex}
                            active={step.span.span_id === selectedSpanId}
                            onOpen={() => select(step.span)}
                          />
                        ) : (
                          <ToolStepView
                            key={step.span.span_id}
                            step={step}
                            active={step.span.span_id === selectedSpanId}
                            onOpen={() => select(step.span)}
                          />
                        ),
                      )}
                    </ol>
                  </>
                )}
              </TabsContent>

              <TabsContent value="spans" className="rounded-lg border bg-card p-3">
                <Waterfall spans={spans} selected={selectedSpanId} onSelect={select} />
              </TabsContent>
            </Tabs>
            {selected && <SpanDetail key={selected} id={selected} onClose={() => select(null)} />}
          </div>
        </>
      )}
    </div>
  )
}
