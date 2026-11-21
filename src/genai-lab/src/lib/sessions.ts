// Claude Code sessions rebuilt from its telemetry. Spans give each turn's structure (the prompt on the
// `claude_code.interaction` root, then model calls and tool calls in order); log events add what spans lack: the
// reply text (`assistant_response`, joined on the model call's request id) and each tool's input (`tool_result`,
// joined on the tool use id).
import type { TimeWindow } from './agent'
import type { LogRecord } from './logs'
import { absolute } from './page-state'
import { field, K, postTraces, type ExploreAnswer, type Fields, type TraceAnswer, type TraceSpan } from './traces'

export const S = {
  conversation: 'attributes.gen_ai.conversation.id',
  prompt: 'attributes.user_prompt',
  sequence: 'attributes.interaction.sequence',
  spanType: 'attributes.span.type',
  requestId: 'attributes.request_id',
  toolName: 'attributes.tool_name',
  toolUseId: 'attributes.tool_use_id',
  command: 'attributes.full_command',
  filePath: 'attributes.file_path',
  success: 'attributes.success',
  decision: 'attributes.decision',
  ttft: 'attributes.ttft_ms',
  stopReason: 'attributes.stop_reason',
  inputTokens: 'attributes.input_tokens',
  outputTokens: 'attributes.output_tokens',
  cacheRead: 'attributes.cache_read_tokens',
  cacheWrite: 'attributes.cache_creation_tokens',
  model: 'attributes.model',
  agentName: 'attributes.agent_name',
} as const

export const INTERACTION = 'claude_code.interaction'

export type TurnRow = {
  traceId: string
  startNs: number
  durationNs: number
  conversation: string
  prompt?: string
  sequence?: number
}

export type Session = {
  id: string
  firstNs: number
  lastNs: number
  turns: TurnRow[]
}

const TURN_COLUMNS = [S.conversation, S.prompt, S.sequence]
const TURN_PAGE = 200
const MAX_TURNS = 5000

// Every turn root in the window, optionally of one session, oldest first.
export async function fetchTurns(w: TimeWindow, conversation?: string, signal?: AbortSignal): Promise<TurnRow[]> {
  const { after, before } = absolute(w)
  const where: { key: string; op: string; values: string[] }[] = [{ key: K.name, op: '=', values: [INTERACTION] }]
  if (conversation) where.push({ key: S.conversation, op: '=', values: [conversation] })
  const rows = { limit: TURN_PAGE, columns: TURN_COLUMNS }
  const out: TurnRow[] = []
  let anchor: string | undefined
  while (out.length < MAX_TURNS) {
    const body = anchor
      ? { explore: { conditions: where, anchor, rows } }
      : { explore: { after, before, conditions: where, rows } }
    const a = await postTraces<ExploreAnswer>(body, signal)
    for (const r of a.rows.items) {
      const seq = Number(field(r.fields, S.sequence))
      out.push({
        traceId: r.trace_id,
        startNs: r.start_ns,
        durationNs: r.duration_ns,
        conversation: field(r.fields, S.conversation) ?? '',
        prompt: field(r.fields, S.prompt),
        sequence: Number.isFinite(seq) ? seq : undefined,
      })
    }
    anchor = a.rows.anchor?.next
    if (!anchor || a.rows.items.length < TURN_PAGE) break
  }
  return out.sort((a, b) => a.startNs - b.startNs)
}

export function groupSessions(turns: TurnRow[]): Session[] {
  const byId = new Map<string, Session>()
  for (const t of turns) {
    const id = t.conversation || '(no session id)'
    let s = byId.get(id)
    if (!s) {
      s = { id, firstNs: t.startNs, lastNs: t.startNs, turns: [] }
      byId.set(id, s)
    }
    s.turns.push(t)
    s.firstNs = Math.min(s.firstNs, t.startNs)
    s.lastNs = Math.max(s.lastNs, t.startNs + t.durationNs)
  }
  return [...byId.values()].sort((a, b) => b.lastNs - a.lastNs)
}

export type ModelStep = {
  kind: 'model'
  spanId: string
  startNs: number
  durationNs: number
  model?: string
  inputTokens?: number
  outputTokens?: number
  cacheRead?: number
  cacheWrite?: number
  ttftMs?: number
  stopReason?: string
  error: boolean
  // Text blocks of the reply; empty when the model answered with tool calls only.
  text: string[]
}

export type ToolStep = {
  kind: 'tool'
  spanId: string
  startNs: number
  durationNs: number
  name: string
  input?: string
  output?: string
  success?: boolean
  decision?: string
  // A subagent's own model and tool calls nest under the tool that started it.
  steps: Step[]
}

export type Step = ModelStep | ToolStep

export type Turn = { prompt?: string; steps: Step[] }

function num(v: string | undefined): number | undefined {
  if (v == null) return undefined
  const n = Number(v)
  return Number.isFinite(n) ? n : undefined
}

type RawEvent = { name?: string; attributes?: Fields; fields?: Fields }

function eventValue(span: TraceSpan, name: string, key: string): string | undefined {
  for (const e of (span.events ?? []) as RawEvent[]) {
    if (e.name !== name) continue
    const v = field(e.attributes ?? e.fields, `events.attributes.${key}`, key)
    if (v != null) return v
  }
  return undefined
}

// The tool input as the model wrote it: the logged JSON, with a shell command shown as the command itself.
export function toolInput(log: LogRecord | undefined, span: TraceSpan): string | undefined {
  const raw = log?.['attributes.tool_input']
  if (raw) {
    try {
      const parsed = JSON.parse(raw)
      if (parsed && typeof parsed === 'object') {
        const keys = Object.keys(parsed)
        if (keys.length === 1 && typeof parsed[keys[0]] === 'string') return parsed[keys[0]]
        // Shell-like tools: the command is what reads; options such as a timeout add little.
        if (typeof parsed.command === 'string') return parsed.command
        return JSON.stringify(parsed, null, 2)
      }
    } catch {
      // Cut by Claude Code past its size limit: shown as sent.
    }
    return raw
  }
  return field(span.fields, S.command, S.filePath)
}

export type LogIndex = { replies: Map<string, string[]>; tools: Map<string, LogRecord> }

// Log records of a session's window, indexed by the ids the spans carry.
export function indexLogs(records: LogRecord[]): LogIndex {
  const replies = new Map<string, string[]>()
  const tools = new Map<string, LogRecord>()
  const sorted = [...records].sort(
    (a, b) => Number(a['attributes.event.sequence'] ?? 0) - Number(b['attributes.event.sequence'] ?? 0),
  )
  for (const r of sorted) {
    const event = r['attributes.event.name']
    if (event === 'assistant_response') {
      const id = r['attributes.request_id']
      const text = r['attributes.response']
      if (!id || !text) continue
      const list = replies.get(id) ?? []
      list.push(text)
      replies.set(id, list)
    } else if (event === 'tool_result') {
      const id = r['attributes.tool_use_id']
      if (id) tools.set(id, r)
    }
  }
  return { replies, tools }
}

// One turn from its trace: spans arrive in tree order with their depth.
export function buildTurn(trace: TraceAnswer, logs: LogIndex): Turn {
  const spans = trace.spans
  const children = new Map<number, number[]>()
  const stack: number[] = []
  spans.forEach((s, i) => {
    const depth = s.depth ?? 0
    stack.length = depth
    const parent = depth > 0 ? stack[depth - 1] : undefined
    if (parent != null) children.set(parent, [...(children.get(parent) ?? []), i])
    stack[depth] = i
  })

  const stepsOf = (i: number): Step[] => {
    const out: Step[] = []
    for (const c of children.get(i) ?? []) {
      const s = spans[c]
      const type = field(s.fields, S.spanType)
      if (type === 'llm_request') {
        const requestId = field(s.fields, S.requestId)
        out.push({
          kind: 'model',
          spanId: s.span_id,
          startNs: s.start_ns,
          durationNs: s.duration_ns,
          model: field(s.fields, S.model, K.requestModel),
          inputTokens: num(field(s.fields, S.inputTokens)),
          outputTokens: num(field(s.fields, S.outputTokens)),
          cacheRead: num(field(s.fields, S.cacheRead)),
          cacheWrite: num(field(s.fields, S.cacheWrite)),
          ttftMs: num(field(s.fields, S.ttft)),
          stopReason: field(s.fields, S.stopReason),
          error: field(s.fields, K.status) === 'ERROR' || field(s.fields, S.success) === 'false',
          text: (requestId && logs.replies.get(requestId)) || [],
        })
      } else if (type === 'tool') {
        const useId = field(s.fields, S.toolUseId)
        const log = useId ? logs.tools.get(useId) : undefined
        let success: boolean | undefined
        let decision: string | undefined
        for (const g of children.get(c) ?? []) {
          const gt = field(spans[g].fields, S.spanType)
          if (gt === 'tool.execution') success = field(spans[g].fields, S.success) !== 'false'
          if (gt === 'tool.blocked_on_user') decision = field(spans[g].fields, S.decision)
        }
        out.push({
          kind: 'tool',
          spanId: s.span_id,
          startNs: s.start_ns,
          durationNs: s.duration_ns,
          name: field(s.fields, S.toolName, 'attributes.gen_ai.tool.name') ?? 'tool',
          input: toolInput(log, s),
          output: eventValue(s, 'tool.output', 'output'),
          success,
          decision,
          steps: stepsOf(c),
        })
      } else {
        // Containers such as the tool's execution span may hold a subagent's calls; lift them to this level.
        out.push(...stepsOf(c))
      }
    }
    return out.sort((a, b) => a.startNs - b.startNs)
  }

  const root = spans.findIndex((s) => field(s.fields, K.name) === INTERACTION)
  if (root < 0) return { steps: [] }
  return { prompt: field(spans[root].fields, S.prompt), steps: stepsOf(root) }
}
