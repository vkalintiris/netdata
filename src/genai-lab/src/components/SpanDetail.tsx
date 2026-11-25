import { useQuery } from '@tanstack/react-query'
import { ChevronRight, Wrench, X } from 'lucide-react'
import { useMemo, useState, type ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { conversationOfSpan, type Message, type Part } from '@/lib/conversation'
import { formatCount, formatSeconds } from '@/lib/format'
import { fetchTrace, field, K, type TraceSpan } from '@/lib/traces'
import { cn } from '@/lib/utils'

const dateFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="truncate font-mono text-sm tabular-nums">{children ?? '–'}</span>
    </div>
  )
}

function count(v: string | undefined) {
  return v == null ? undefined : formatCount(Number(v))
}

function PartView({ part }: { part: Part }) {
  if (part.kind === 'text') return <p className="text-sm whitespace-pre-wrap">{part.text}</p>
  if (part.kind === 'toolCall') {
    return (
      <div className="rounded-md border bg-muted/40 p-2">
        <div className="mb-1 flex items-center gap-1.5 text-xs font-medium">
          <Wrench className="size-3.5" />
          {part.name ?? 'tool call'}
        </div>
        {part.arguments && <pre className="overflow-x-auto font-mono text-xs whitespace-pre-wrap">{part.arguments}</pre>}
      </div>
    )
  }
  if (part.kind === 'reasoning') {
    return (
      <div className="border-l-2 border-dashed pl-2">
        <div className="mb-1 text-xs text-muted-foreground">reasoning</div>
        <p className="text-sm whitespace-pre-wrap text-muted-foreground italic">{part.text}</p>
      </div>
    )
  }
  if (part.kind === 'toolResult') {
    return (
      <div className="rounded-md border border-dashed p-2">
        <div className="mb-1 text-xs text-muted-foreground">tool result</div>
        <pre className="overflow-x-auto font-mono text-xs whitespace-pre-wrap">{part.text}</pre>
      </div>
    )
  }
  return (
    <div className="rounded-md border border-dashed p-2">
      <div className="mb-1 text-xs text-muted-foreground">{part.type}</div>
      <pre className="overflow-x-auto font-mono text-xs whitespace-pre-wrap">{part.text}</pre>
    </div>
  )
}

const ROLE_STYLE: Record<string, string> = {
  system: 'border-l-[var(--chart-4)]',
  user: 'border-l-[var(--chart-1)]',
  assistant: 'border-l-[var(--chart-2)]',
  tool: 'border-l-[var(--chart-3)]',
}

function MessageView({ message }: { message: Message }) {
  // Some instrumentations send a tool's result as a user message; it reads as the tool speaking.
  const toolOnly = message.parts.length > 0 && message.parts.every((p) => p.kind === 'toolResult')
  const role = toolOnly ? 'tool' : (message.role ?? (message.section === 'output' ? 'assistant' : 'input'))
  return (
    <div className={cn('flex flex-col gap-2 border-l-2 pl-3', ROLE_STYLE[role] ?? 'border-l-border')}>
      <div className="flex items-center gap-2 text-xs">
        <span className="font-medium capitalize">{role}</span>
        {message.section === 'output' && <span className="text-muted-foreground">answer</span>}
        {message.finishReason && <span className="text-muted-foreground">· {message.finishReason}</span>}
      </div>
      {message.notice && <p className="text-xs text-[var(--error)]">{message.notice}</p>}
      {message.parts.map((part, i) => (
        <PartView key={i} part={part} />
      ))}
    </div>
  )
}

function Waterfall({ spans, selected }: { spans: TraceSpan[]; selected: string }) {
  const sorted = spans
  const start = Math.min(...sorted.map((s) => s.start_ns))
  const end = Math.max(...sorted.map((s) => s.start_ns + s.duration_ns))
  const span = Math.max(1, end - start)
  return (
    <ul className="flex flex-col gap-0.5">
      {sorted.map((s) => {
        const left = ((s.start_ns - start) / span) * 100
        const width = Math.max(0.5, (s.duration_ns / span) * 100)
        const error = field(s.fields, K.status) === 'ERROR'
        const model = field(s.fields, K.call) !== undefined
        return (
          <li
            key={s.span_id}
            className={cn(
              'grid grid-cols-[200px_1fr_72px] items-center gap-2 rounded px-1 text-xs',
              s.span_id === selected && 'bg-muted',
            )}
          >
            <span
              className="truncate"
              style={{ paddingLeft: `${Math.min(s.depth ?? 0, 8) * 10}px` }}
              title={field(s.fields, K.name)}
            >
              {field(s.fields, K.name) ?? s.span_id}
            </span>
            <span className="relative h-3">
              <span
                className={cn(
                  'absolute inset-y-0 rounded-sm',
                  error ? 'bg-[var(--error)]' : model ? 'bg-[var(--chart-2)]' : 'bg-[var(--chart-1)]/60',
                )}
                style={{ left: `${left}%`, width: `${width}%` }}
              />
            </span>
            <span className="text-right font-mono whitespace-nowrap tabular-nums text-muted-foreground">
              {formatSeconds(s.duration_ns / 1e9)}
            </span>
          </li>
        )
      })}
    </ul>
  )
}

export function SpanDetail({ id, onClose }: { id: string; onClose: () => void }) {
  const [traceId, spanId, startNs] = id.split(':')
  const trace = useQuery({
    queryKey: ['trace', traceId, startNs],
    queryFn: ({ signal }) => fetchTrace(traceId, Number(startNs), signal),
  })
  const span = trace.data?.spans.find((s) => s.span_id === spanId)
  // A call recorded by two instrumentations is two spans, and often only one of them carries the messages; an
  // overlapping span of the same trace that has them stands in.
  const { conversation, source } = useMemo(() => {
    if (!span || !trace.data) return { conversation: null, source: null }
    const own = conversationOfSpan(span)
    if (own) return { conversation: own, source: null }
    const end = span.start_ns + span.duration_ns
    let best: { c: ReturnType<typeof conversationOfSpan>; s: TraceSpan } | null = null
    for (const other of trace.data.spans) {
      if (other.span_id === span.span_id) continue
      if (other.start_ns > end || other.start_ns + other.duration_ns < span.start_ns) continue
      const c = conversationOfSpan(other)
      if (!c) continue
      if (!best || Math.abs(other.start_ns - span.start_ns) < Math.abs(best.s.start_ns - span.start_ns)) {
        best = { c, s: other }
      }
    }
    return best ? { conversation: best.c, source: best.s } : { conversation: null, source: null }
  }, [span, trace.data])
  const [showAll, setShowAll] = useState(false)

  const f = (...keys: string[]) => field(span?.fields, ...keys)
  const error = f(K.outcome) === 'error'
  const tool = f(K.kind) === 'tool'
  const finishReasons =
    f(K.finishReasons) ??
    (conversation?.messages
      .map((m) => (m.section === 'output' ? m.finishReason : undefined))
      .filter(Boolean)
      .join(', ') ||
      undefined)

  return (
    <aside className="flex max-h-[calc(100vh-7rem)] min-w-0 flex-col self-start overflow-hidden rounded-lg border bg-card xl:sticky xl:top-16">
      <header className="flex items-start gap-2 border-b p-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h3 className="truncate font-medium">
              {tool ? (f(K.tool, K.name) ?? 'Tool call') : (f(K.requestModel) ?? 'Model call')}
            </h3>
            {tool && <Badge variant="outline">tool</Badge>}
            {error && <Badge variant="destructive">error</Badge>}
          </div>
          <p className="truncate text-xs text-muted-foreground">
            {f(K.service)} · {span ? dateFmt.format(span.start_ns / 1e6) : ''}
          </p>
          {span && (
            <p className="mt-1 flex flex-wrap gap-x-3 font-mono text-xs tabular-nums">
              <span>{formatSeconds(span.duration_ns / 1e9)}</span>
              {!tool && (
                <span>
                  {count(f(K.inputTokens)) ?? '–'} in · {count(f(K.outputTokens)) ?? '–'} out
                </span>
              )}
              {finishReasons && <span className="text-muted-foreground">{finishReasons}</span>}
            </p>
          )}
        </div>
        <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label="Close">
          <X />
        </Button>
      </header>

      {trace.error ? (
        <p className="p-4 text-sm text-destructive">{String((trace.error as Error).message)}</p>
      ) : !trace.data ? (
        <div className="flex flex-col gap-2 p-4">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : !span ? (
        <p className="p-4 text-sm text-muted-foreground">This span is no longer in the trace answer.</p>
      ) : (
        <Tabs defaultValue={conversation ? 'conversation' : 'summary'} className="min-h-0 flex-1 gap-0">
          <TabsList className="mx-3 mt-3">
            <TabsTrigger value="conversation">Conversation</TabsTrigger>
            <TabsTrigger value="summary">Summary</TabsTrigger>
            <TabsTrigger value="trace">Trace</TabsTrigger>
          </TabsList>

          <TabsContent value="conversation" className="min-h-0 overflow-y-auto p-3">
            {conversation ? (
              <div className="flex flex-col gap-4">
                {source && (
                  <p className="rounded-md bg-muted px-2 py-1 text-xs text-muted-foreground">
                    From the span “{field(source.fields, K.name) ?? source.span_id}”
                    {field(source.fields, 'scope.name') ? ` (${field(source.fields, 'scope.name')})` : ''}: this span
                    carries no messages, and another instrumentation recorded the same call with them.
                  </p>
                )}
                {conversation.tools.length > 0 && (
                  <div className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
                    Tools offered:
                    {conversation.tools.map((t) => (
                      <Badge key={t} variant="outline">
                        {t}
                      </Badge>
                    ))}
                  </div>
                )}
                {conversation.notices.map((n) => (
                  <p key={n} className="text-xs text-[var(--error)]">
                    {n}
                  </p>
                ))}
                {conversation.messages.map((m, i) => (
                  <MessageView key={i} message={m} />
                ))}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                {tool
                  ? 'A tool call has no conversation of its own; its input and output, when the producer records them, are among the attributes on the Summary tab.'
                  : 'This call carries no messages. Most instrumentations record them only when content capture is turned on.'}
              </p>
            )}
          </TabsContent>

          <TabsContent value="summary" className="min-h-0 overflow-y-auto p-3">
            <div className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
              <Fact label="Duration">{formatSeconds(span.duration_ns / 1e9)}</Fact>
              {tool && <Fact label="Tool">{f(K.tool)}</Fact>}
              {!tool && (
                <>
                  <Fact label="Operation">{f(K.operation)}</Fact>
                  <Fact label="Provider">{f(K.provider)}</Fact>
                  <Fact label="Requested model">{f(K.requestModel)}</Fact>
                  <Fact label="Answering model">{f(K.responseModel)}</Fact>
                  <Fact label="Finish reasons">{finishReasons}</Fact>
                  <Fact label="Input tokens">{count(f(K.inputTokens))}</Fact>
                  <Fact label="Output tokens">{count(f(K.outputTokens))}</Fact>
                  <Fact label="Reasoning tokens">{count(f(K.reasoning))}</Fact>
                  <Fact label="Cache read">{count(f(K.cacheRead))}</Fact>
                  <Fact label="Cache write">{count(f(K.cacheWrite))}</Fact>
                </>
              )}
              {error && <Fact label="Error">{f(K.errorType, K.statusMessage)}</Fact>}
            </div>
            <button
              type="button"
              className="mt-4 flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
              onClick={() => setShowAll((v) => !v)}
            >
              <ChevronRight className={cn('size-3.5 transition-transform', showAll && 'rotate-90')} />
              All attributes ({span.fields.length})
            </button>
            {showAll && (
              <dl className="mt-2 grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)] gap-x-3 gap-y-1 font-mono text-xs">
                {span.fields.map(([k, v], i) => (
                  <div key={i} className="contents">
                    <dt className="truncate text-muted-foreground" title={k}>
                      {k}
                    </dt>
                    <dd className="break-all">{v.length > 400 ? `${v.slice(0, 400)}…` : v}</dd>
                  </div>
                ))}
              </dl>
            )}
          </TabsContent>

          <TabsContent value="trace" className="min-h-0 overflow-y-auto p-3">
            <p className="mb-2 font-mono text-xs text-muted-foreground">{traceId}</p>
            <Waterfall spans={trace.data.spans} selected={spanId} />
          </TabsContent>
        </Tabs>
      )}
    </aside>
  )
}
