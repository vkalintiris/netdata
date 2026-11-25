import { describe, expect, it } from 'vitest'
import { transcriptOf } from './transcript'
import { K, type Fields, type TraceSpan } from './traces'

type Wire = { role: string; parts: { type: string; content?: string; name?: string; response?: string }[] }

const text = (role: string, content: string): Wire => ({ role, parts: [{ type: 'text', content }] })

function call(id: string, startS: number, input: Wire[], output: Wire[], extra: Fields = []): TraceSpan {
  return {
    span_id: id,
    start_ns: startS * 1e9,
    duration_ns: 1e9,
    fields: [
      [K.kind, 'call'],
      [K.call, 'counted'],
      ['attributes.gen_ai.input.messages', JSON.stringify(input)],
      ['attributes.gen_ai.output.messages', JSON.stringify(output)],
      ...extra,
    ],
  }
}

function tool(id: string, startS: number): TraceSpan {
  return { span_id: id, start_ns: startS * 1e9, duration_ns: 1e6, fields: [[K.kind, 'tool']] }
}

// What a step shows, as plain text.
function shown(spans: TraceSpan[]) {
  return transcriptOf(spans).map((step) => {
    if (step.kind === 'tool') return { tool: step.span.span_id }
    const words = (messages: typeof step.added) =>
      messages.map((m) => `${m.role}: ${m.parts.map((p) => ('text' in p ? p.text : p.kind)).join(' ')}`)
    return { call: step.span.span_id, added: words(step.added), answer: words(step.answer), startsOver: step.startsOver }
  })
}

describe('transcriptOf', () => {
  const system = text('system', 'Guess the number.')
  const ask = text('user', 'Your first guess?')
  const fifty = text('assistant', '50')
  const lower = text('user', 'lower')
  const twentyFive = text('assistant', '25')

  it('shows each call only the messages new since the call it continues', () => {
    expect(
      shown([
        // Out of order, as a trace answer may list them.
        call('2', 20, [system, ask, fifty, lower], [twentyFive]),
        tool('t', 15),
        call('1', 10, [system, ask], [fifty]),
      ]),
    ).toEqual([
      { call: '1', added: ['system: Guess the number.', 'user: Your first guess?'], answer: ['assistant: 50'], startsOver: false },
      { tool: 't' },
      { call: '2', added: ['user: lower'], answer: ['assistant: 25'], startsOver: false },
    ])
  })

  it('shows the whole input of a call that continues no earlier one', () => {
    const other = [text('system', 'Summarize.'), text('user', 'The game so far.')]
    expect(shown([call('1', 10, [system, ask], [fifty]), call('2', 20, other, [text('assistant', 'Done.')])])).toEqual([
      { call: '1', added: ['system: Guess the number.', 'user: Your first guess?'], answer: ['assistant: 50'], startsOver: false },
      { call: '2', added: ['system: Summarize.', 'user: The game so far.'], answer: ['assistant: Done.'], startsOver: true },
    ])
  })

  it('continues the right call when two conversations take turns', () => {
    const otherSystem = text('system', 'Judge the guess.')
    const judge = text('user', 'Is 50 right?')
    const no = text('assistant', 'no')
    expect(
      shown([
        call('a1', 10, [system, ask], [fifty]),
        call('b1', 11, [otherSystem, judge], [no]),
        call('a2', 12, [system, ask, fifty, lower], [twentyFive]),
      ]).map((s) => ('call' in s ? [s.call, s.added, s.startsOver] : s)),
    ).toEqual([
      ['a1', ['system: Guess the number.', 'user: Your first guess?'], false],
      ['b1', ['system: Judge the guess.', 'user: Is 50 right?'], true],
      ['a2', ['user: lower'], false],
    ])
  })

  it('leaves out the second span of a call and reads a call without messages as a step', () => {
    const bare: TraceSpan = { span_id: 'x', start_ns: 30e9, duration_ns: 1e9, fields: [[K.kind, 'call'], [K.call, 'counted']] }
    const second: TraceSpan = { span_id: 'y', start_ns: 31e9, duration_ns: 1e9, fields: [[K.kind, 'call'], [K.call, 'duplicate']] }
    const steps = transcriptOf([bare, second])
    expect(steps.map((s) => [s.span.span_id, s.kind === 'call' && s.hasContent])).toEqual([['x', false]])
  })
})
