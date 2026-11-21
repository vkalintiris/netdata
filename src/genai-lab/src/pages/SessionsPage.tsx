import { useQuery } from '@tanstack/react-query'
import { Bot, ChevronRight, CircleAlert, User, Wrench } from 'lucide-react'
import { useState } from 'react'
import { useSearchParams } from 'react-router'
import { Skeleton } from '@/components/ui/skeleton'
import { formatCount, formatSeconds } from '@/lib/format'
import { fetchLogs } from '@/lib/logs'
import { usePageState } from '@/lib/page-state'
import {
  buildTurn,
  fetchTurns,
  groupSessions,
  indexLogs,
  type LogIndex,
  type ModelStep,
  type Session,
  type Step,
  type ToolStep,
  type TurnRow,
} from '@/lib/sessions'
import { fetchTrace } from '@/lib/traces'
import { cn } from '@/lib/utils'

const REFRESH_MS = 15_000

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
const dateFmt = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })

function SessionList({
  sessions,
  selected,
  onSelect,
}: {
  sessions: Session[]
  selected: string | null
  onSelect: (id: string) => void
}) {
  return (
    <ul className="flex flex-col gap-1">
      {sessions.map((s) => (
        <li key={s.id}>
          <button
            type="button"
            onClick={() => onSelect(s.id)}
            className={cn(
              'flex w-full flex-col gap-1 rounded-md border p-2 text-left hover:bg-muted/60',
              s.id === selected && 'border-primary bg-muted',
            )}
          >
            <span className="flex items-baseline gap-2 text-xs text-muted-foreground">
              <span>{dateFmt.format(s.lastNs / 1e6)}</span>
              <span>
                · {s.turns.length} turn{s.turns.length === 1 ? '' : 's'}
              </span>
              <span className="ml-auto font-mono">{s.id.slice(0, 8)}</span>
            </span>
            <span className="line-clamp-2 text-sm">{s.turns[0]?.prompt ?? '(no prompt recorded)'}</span>
          </button>
        </li>
      ))}
    </ul>
  )
}

function Collapsible({ text, lines = 6 }: { text: string; lines?: number }) {
  const [open, setOpen] = useState(false)
  const all = text.split('\n')
  const long = all.length > lines || text.length > 800
  const shown = open || !long ? text : all.slice(0, lines).join('\n').slice(0, 800)
  return (
    <div>
      <pre className="max-h-[480px] overflow-auto font-mono text-xs whitespace-pre-wrap break-all">{shown}</pre>
      {long && (
        <button
          type="button"
          className="mt-1 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setOpen((v) => !v)}
        >
          {open ? 'Show less' : `Show all (${all.length} lines)`}
        </button>
      )}
    </div>
  )
}

function ModelView({ step }: { step: ModelStep }) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 font-mono text-xs text-muted-foreground tabular-nums">
        <span className="flex items-center gap-1.5 font-sans font-medium text-foreground">
          <Bot className="size-3.5" />
          {step.model ?? 'model call'}
        </span>
        <span>{formatSeconds(step.durationNs / 1e9)}</span>
        <span>
          {formatCount(step.inputTokens)} in · {formatCount(step.outputTokens)} out
        </span>
        {step.cacheRead != null && <span>{formatCount(step.cacheRead)} cache read</span>}
        {step.stopReason && <span>{step.stopReason}</span>}
        {step.error && (
          <span className="flex items-center gap-1 text-[var(--error)]">
            <CircleAlert className="size-3.5" /> failed
          </span>
        )}
      </div>
      {step.text.map((t, i) => (
        <p key={i} className="border-l-2 border-l-[var(--chart-2)] pl-3 text-sm whitespace-pre-wrap">
          {t}
        </p>
      ))}
    </div>
  )
}

function ToolView({ step, logs }: { step: ToolStep; logs: LogIndex }) {
  const [open, setOpen] = useState(false)
  const preview = step.input?.slice(0, 300).replace(/\s+/g, ' ')
  return (
    <div className="rounded-md border bg-muted/30">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs"
        onClick={() => setOpen((v) => !v)}
      >
        <ChevronRight className={cn('size-3.5 shrink-0 transition-transform', open && 'rotate-90')} />
        <Wrench className="size-3.5 shrink-0" />
        <span className="shrink-0 font-medium">{step.name}</span>
        {preview && <span className="truncate font-mono text-muted-foreground">{preview}</span>}
        <span className="ml-auto flex shrink-0 gap-2 font-mono text-muted-foreground tabular-nums">
          {step.success === false && <span className="text-[var(--error)]">failed</span>}
          {step.decision && step.decision !== 'accept' && <span>{step.decision}</span>}
          {formatSeconds(step.durationNs / 1e9)}
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-2 border-t p-2">
          {step.input ? (
            <div>
              <div className="mb-1 text-xs text-muted-foreground">input</div>
              <Collapsible text={step.input} />
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">No input recorded.</p>
          )}
          {step.output ? (
            <div>
              <div className="mb-1 text-xs text-muted-foreground">output</div>
              <Collapsible text={step.output} />
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">No output recorded.</p>
          )}
        </div>
      )}
      {step.steps.length > 0 && (
        <div className="border-t p-2 pl-4">
          <Steps steps={step.steps} logs={logs} />
        </div>
      )}
    </div>
  )
}

function Steps({ steps, logs }: { steps: Step[]; logs: LogIndex }) {
  return (
    <div className="flex flex-col gap-2">
      {steps.map((s) =>
        s.kind === 'model' ? <ModelView key={s.spanId} step={s} /> : <ToolView key={s.spanId} step={s} logs={logs} />,
      )}
    </div>
  )
}

function TurnView({ row, index, logs }: { row: TurnRow; index: number; logs: LogIndex }) {
  const trace = useQuery({
    queryKey: ['trace', row.traceId, String(row.startNs)],
    queryFn: ({ signal }) => fetchTrace(row.traceId, row.startNs, signal),
  })
  const turn = trace.data ? buildTurn(trace.data, logs) : null
  const models = turn?.steps.filter((s) => s.kind === 'model').length ?? 0
  const tools = turn?.steps.filter((s) => s.kind === 'tool').length ?? 0
  return (
    <section className="flex flex-col gap-3 rounded-lg border bg-card p-3">
      <header className="flex items-baseline gap-2 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">Turn {row.sequence ?? index + 1}</span>
        <span>{timeFmt.format(row.startNs / 1e6)}</span>
        <span>· {formatSeconds(row.durationNs / 1e9)}</span>
        {turn && (
          <span>
            · {models} model call{models === 1 ? '' : 's'} · {tools} tool call{tools === 1 ? '' : 's'}
          </span>
        )}
      </header>
      <div className="flex gap-2 border-l-2 border-l-[var(--chart-1)] pl-3">
        <User className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <p className="text-sm whitespace-pre-wrap">{turn?.prompt ?? row.prompt ?? '(no prompt recorded)'}</p>
      </div>
      {trace.error ? (
        <p className="text-sm text-destructive">{String((trace.error as Error).message)}</p>
      ) : !turn ? (
        <Skeleton className="h-10 w-full" />
      ) : (
        <Steps steps={turn.steps} logs={logs} />
      )}
    </section>
  )
}

const EMPTY_LOGS: LogIndex = { replies: new Map(), tools: new Map() }

function SessionView({ session }: { session: Session }) {
  const after = Math.floor(session.firstNs / 1e9) - 5
  const before = Math.ceil(session.lastNs / 1e9) + 5
  const logs = useQuery({
    queryKey: ['session-logs', session.id, after, before],
    queryFn: async ({ signal }) =>
      indexLogs(
        await fetchLogs(
          after,
          before,
          {
            'attributes.event.name': ['assistant_response', 'tool_result'],
            'resource.attributes.service.name': ['claude-code'],
          },
          signal,
        ),
      ),
  })
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <header className="flex flex-wrap items-baseline gap-x-3 text-xs text-muted-foreground">
        <span className="font-mono text-sm text-foreground">{session.id}</span>
        <span>
          {dateFmt.format(session.firstNs / 1e6)} – {timeFmt.format(session.lastNs / 1e6)}
        </span>
        <span>{session.turns.length} turns in this window</span>
        {logs.error && <span className="text-destructive">replies unavailable: {(logs.error as Error).message}</span>}
      </header>
      {session.turns.map((t, i) => (
        <TurnView key={t.traceId} row={t} index={i} logs={logs.data ?? EMPTY_LOGS} />
      ))}
    </div>
  )
}

export function SessionsPage() {
  const { window } = usePageState()
  const [params, setParams] = useSearchParams()
  const selectedId = params.get('session')
  const turns = useQuery({
    queryKey: ['turns', window],
    queryFn: ({ signal }) => fetchTurns(window, undefined, signal),
    refetchInterval: window.after < 0 ? REFRESH_MS : false,
    placeholderData: (prev) => prev,
  })
  const sessions = turns.data ? groupSessions(turns.data) : []
  const selected = sessions.find((s) => s.id === selectedId) ?? null
  const select = (id: string) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev)
      next.set('session', id)
      return next
    })

  return (
    <div className="grid gap-4 lg:grid-cols-[340px_minmax(0,1fr)]">
      <aside className="flex flex-col gap-2 self-start lg:sticky lg:top-16 lg:max-h-[calc(100vh-6rem)] lg:overflow-y-auto">
        <p className="text-xs text-muted-foreground">
          Claude Code sessions with a turn in this window, rebuilt from its spans and log events.
        </p>
        {turns.error ? (
          <p className="text-sm text-destructive">{String((turns.error as Error).message)}</p>
        ) : !turns.data ? (
          <Skeleton className="h-24 w-full" />
        ) : sessions.length === 0 ? (
          <p className="text-sm text-muted-foreground">No Claude Code turns in this window.</p>
        ) : (
          <SessionList sessions={sessions} selected={selected?.id ?? null} onSelect={select} />
        )}
      </aside>
      <div className="min-w-0">
        {selected ? (
          <SessionView key={selected.id} session={selected} />
        ) : (
          <p className="text-sm text-muted-foreground">Pick a session.</p>
        )}
      </div>
    </div>
  )
}
