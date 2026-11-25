import { describe, expect, it } from 'vitest'
import { groupRuns } from './runs'
import { K, type Fields, type SpanRow } from './traces'

function call(trace: string, span: string, startS: number, durationS: number, fields: Fields): SpanRow {
  return { trace_id: trace, span_id: span, start_ns: startS * 1e9, duration_ns: durationS * 1e9, fields }
}

describe('groupRuns', () => {
  it('sums the calls of a trace and lists runs latest first', () => {
    const a1 = call('a', '1', 10, 2, [
      [K.service, 'shop'],
      [K.requestModel, 'small'],
      [K.provider, 'openai'],
      [K.inputTokens, '100'],
      [K.outputTokens, '10'],
    ])
    const a2 = call('a', '2', 15, 3, [
      [K.service, 'shop'],
      [K.requestModel, 'large'],
      [K.provider, 'openai'],
      [K.outcome, 'error'],
      [K.inputTokens, '200'],
    ])
    const b1 = call('b', '1', 12, 1, [[K.service, 'bot']])
    // Newest first, as the Function lists them, with one row repeated by a second page.
    expect(groupRuns([a2, b1, a1, a1])).toEqual([
      {
        traceId: 'b',
        service: 'bot',
        startNs: 12e9,
        endNs: 13e9,
        inCallsNs: 1e9,
        calls: [b1],
        failed: 0,
        models: [],
        providers: [],
        tokensIn: null,
        tokensOut: null,
      },
      {
        traceId: 'a',
        service: 'shop',
        startNs: 10e9,
        endNs: 18e9,
        inCallsNs: 5e9,
        calls: [a1, a2],
        failed: 1,
        models: ['small', 'large'],
        providers: ['openai'],
        tokensIn: 300,
        tokensOut: 10,
      },
    ])
  })
  it('ends a run at its longest call, not its last one', () => {
    const [run] = groupRuns([call('a', '1', 10, 20, []), call('a', '2', 12, 1, [])])
    expect([run.startNs, run.endNs, run.inCallsNs]).toEqual([10e9, 30e9, 21e9])
  })
})
