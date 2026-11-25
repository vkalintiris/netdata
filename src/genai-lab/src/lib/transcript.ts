// A trace read as one chat: its model calls and tool calls in order, each call showing only the messages that are
// new since the call it continues.
import { conversationOfSpan, type Conversation, type Message } from './conversation'
import { field, K, NOT_COUNTED, type TraceSpan } from './traces'

export type CallStep = {
  kind: 'call'
  span: TraceSpan
  // Input messages no earlier call of the trace sent, oldest first.
  added: Message[]
  answer: Message[]
  // The call continues no earlier call of the trace: its input is shown whole.
  startsOver: boolean
  // False when neither the span nor another span of the same call carries messages.
  hasContent: boolean
  // The span the messages were read from, when it is not the call's own.
  contentFrom: TraceSpan | null
  notices: string[]
}

export type ToolStep = { kind: 'tool'; span: TraceSpan }

export type Step = CallStep | ToolStep

export function isToolSpan(span: TraceSpan): boolean {
  return field(span.fields, K.kind) === 'tool'
}

// A model call the charts count: the second span of a call that two instrumentations report is left out, and so is
// a gateway's span of a call it forwarded.
export function isCallSpan(span: TraceSpan): boolean {
  return field(span.fields, K.kind) === 'call' && !NOT_COUNTED.includes(field(span.fields, K.call) ?? '')
}

export function isGatewayCallSpan(span: TraceSpan): boolean {
  return field(span.fields, K.kind) === 'call' && field(span.fields, K.call) === 'gateway'
}

// A call recorded by two instrumentations is two spans, and often only one of them carries the messages; an
// overlapping span of the same trace that has them stands in.
export function conversationFor(
  span: TraceSpan,
  spans: TraceSpan[],
): { conversation: Conversation | null; source: TraceSpan | null } {
  const own = conversationOfSpan(span)
  if (own) return { conversation: own, source: null }

  const end = span.start_ns + span.duration_ns
  let best: { conversation: Conversation; span: TraceSpan } | null = null
  for (const other of spans) {
    if (other.span_id === span.span_id) continue
    // Another counted call has its own step; it never lends its messages.
    if (isCallSpan(other) || isToolSpan(other)) continue
    if (other.start_ns > end || other.start_ns + other.duration_ns < span.start_ns) continue
    const conversation = conversationOfSpan(other)
    if (!conversation) continue
    if (!best || Math.abs(other.start_ns - span.start_ns) < Math.abs(best.span.start_ns - span.start_ns)) {
      best = { conversation, span: other }
    }
  }
  return best ? { conversation: best.conversation, source: best.span } : { conversation: null, source: null }
}

function key(message: Message): string {
  return JSON.stringify([message.role, message.parts])
}

export function transcriptOf(spans: TraceSpan[]): Step[] {
  // A trace only a gateway reported (the application behind it sends nothing) reads from the gateway's calls.
  const isStepCall = spans.some(isCallSpan) ? isCallSpan : isGatewayCallSpan
  const ordered = spans.filter((s) => isStepCall(s) || isToolSpan(s)).sort((a, b) => a.start_ns - b.start_ns)
  const steps: Step[] = []
  // The input of every earlier call, as message keys.
  const earlier: string[][] = []

  for (const span of ordered) {
    if (isToolSpan(span)) {
      steps.push({ kind: 'tool', span })
      continue
    }

    const { conversation, source } = conversationFor(span, spans)
    const messages = conversation?.messages ?? []
    const input = messages.filter((m) => m.section === 'input')
    const answer = messages.filter((m) => m.section === 'output')
    const keys = input.map(key)

    // The call continues the earlier call whose whole input opens this one; the longest such input wins.
    let continued = 0
    for (const other of earlier) {
      if (other.length <= continued || other.length > keys.length) continue
      if (other.every((k, i) => k === keys[i])) continued = other.length
    }
    let added = input.slice(continued)
    // What follows the continued input begins with that call's answer, sent back to the model: already shown.
    if (continued > 0) {
      let replayed = 0
      while (replayed < added.length && added[replayed].role === 'assistant') replayed += 1
      added = added.slice(replayed)
    }
    if (keys.length) earlier.push(keys)

    steps.push({
      kind: 'call',
      span,
      added,
      answer,
      startsOver: continued === 0 && earlier.length > 1 && input.length > 0,
      hasContent: messages.length > 0,
      contentFrom: source,
      notices: conversation?.notices ?? [],
    })
  }
  return steps
}
