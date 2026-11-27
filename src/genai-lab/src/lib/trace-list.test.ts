import { describe, expect, it } from 'vitest'
import { groupTraces } from './trace-list'
import { K, type Fields, type SpanRow } from './traces'

function span(trace: string, id: string, startS: number, durationS: number, fields: Fields): SpanRow {
  return { trace_id: trace, span_id: id, start_ns: startS * 1e9, duration_ns: durationS * 1e9, fields }
}

describe('groupTraces', () => {
  it('adds up the calls and tools of a trace and lists traces latest first', () => {
    const a1 = span('a', '1', 10, 2, [
      [K.service, 'shop'],
      [K.kind, 'call'],
      [K.requestModel, 'small'],
      [K.provider, 'openai'],
      [K.inputTokens, '100'],
      [K.outputTokens, '10'],
      [K.priced, 'yes'],
      [K.cost, '2000000'],
    ])
    const a2 = span('a', '2', 13, 1, [
      [K.service, 'shop'],
      [K.kind, 'tool'],
      [K.tool, 'search'],
      [K.outcome, 'error'],
    ])
    const a3 = span('a', '3', 15, 3, [
      [K.service, 'shop'],
      [K.kind, 'call'],
      [K.requestModel, 'large'],
      [K.provider, 'openai'],
      [K.outcome, 'error'],
      [K.inputTokens, '200'],
      [K.priced, 'no'],
    ])
    const b1 = span('b', '1', 12, 1, [[K.service, 'bot']])
    // Newest first, as the Function lists them, with one row repeated by a second page.
    expect(groupTraces([a3, a2, b1, a1, a1])).toEqual([
      {
        traceId: 'b',
        service: 'bot',
        startNs: 12e9,
        endNs: 13e9,
        inCallsNs: 1e9,
        spans: [b1],
        calls: 1,
        failedCalls: 0,
        tools: 0,
        failedTools: 0,
        models: [],
        providers: [],
        tokensIn: null,
        tokensOut: null,
        usd: null,
        unpriced: 0,
      },
      {
        traceId: 'a',
        service: 'shop',
        startNs: 10e9,
        endNs: 18e9,
        // The tool's second is wall time, not time in calls.
        inCallsNs: 5e9,
        spans: [a1, a2, a3],
        calls: 2,
        failedCalls: 1,
        tools: 1,
        failedTools: 1,
        models: ['small', 'large'],
        providers: ['openai'],
        tokensIn: 300,
        tokensOut: 10,
        usd: 0.002,
        unpriced: 1,
      },
    ])
  })
  it('ends a trace at its longest span, not its last one', () => {
    const [trace] = groupTraces([span('a', '1', 10, 20, []), span('a', '2', 12, 1, [])])
    expect([trace.startNs, trace.endNs, trace.inCallsNs]).toEqual([10e9, 30e9, 21e9])
  })
})
