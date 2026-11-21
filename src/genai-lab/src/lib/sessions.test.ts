import { describe, expect, it } from 'vitest'
import { buildTurn, groupSessions, indexLogs, toolInput } from './sessions'
import type { TraceSpan } from './traces'

function span(id: string, depth: number, start: number, fields: [string, string][], events?: unknown[]): TraceSpan {
  return { span_id: id, depth, start_ns: start, duration_ns: 10, fields, events: events as TraceSpan['events'] }
}

describe('buildTurn', () => {
  it('orders model and tool calls, joins replies and tool inputs, nests a subagent', () => {
    const spans = [
      span('root', 0, 0, [
        ['name', 'claude_code.interaction'],
        ['attributes.user_prompt', 'find the bug'],
      ]),
      span('m1', 1, 1, [
        ['attributes.span.type', 'llm_request'],
        ['attributes.model', 'opus'],
        ['attributes.request_id', 'req_1'],
        ['attributes.input_tokens', '4'],
      ]),
      span('t1', 1, 2, [
        ['attributes.span.type', 'tool'],
        ['attributes.tool_name', 'Agent'],
        ['attributes.tool_use_id', 'tu_1'],
      ], [{ name: 'tool.output', attributes: [['events.attributes.output', 'report']] }]),
      span('t1b', 2, 3, [
        ['attributes.span.type', 'tool.blocked_on_user'],
        ['attributes.decision', 'accept'],
      ]),
      span('t1e', 2, 4, [
        ['attributes.span.type', 'tool.execution'],
        ['attributes.success', 'true'],
      ]),
      span('sm', 3, 5, [
        ['attributes.span.type', 'llm_request'],
        ['attributes.model', 'haiku'],
      ]),
      span('m2', 1, 9, [
        ['attributes.span.type', 'llm_request'],
        ['attributes.request_id', 'req_2'],
      ]),
    ]
    const logs = indexLogs([
      { 'attributes.event.name': 'assistant_response', 'attributes.request_id': 'req_2', 'attributes.response': 'done', 'attributes.event.sequence': '3' },
      { 'attributes.event.name': 'tool_result', 'attributes.tool_use_id': 'tu_1', 'attributes.tool_input': '{"prompt":"look"}' },
    ])
    const turn = buildTurn({ trace_id: 'x', spans }, logs)
    expect(turn.prompt).toBe('find the bug')
    expect(turn.steps.map((s) => s.spanId)).toEqual(['m1', 't1', 'm2'])
    const tool = turn.steps[1]
    expect(tool.kind === 'tool' && { input: tool.input, output: tool.output, success: tool.success, decision: tool.decision }).toEqual({
      input: 'look',
      output: 'report',
      success: true,
      decision: 'accept',
    })
    expect(tool.kind === 'tool' && tool.steps.map((s) => s.spanId)).toEqual(['sm'])
    expect(turn.steps[2].kind === 'model' && turn.steps[2].text).toEqual(['done'])
    expect(turn.steps[0].kind === 'model' && turn.steps[0].text).toEqual([])
  })
})

describe('toolInput', () => {
  const bare = span('t', 0, 0, [['attributes.full_command', 'ls']])
  it('shows a shell command as the command', () => {
    expect(toolInput({ 'attributes.tool_input': '{"command":"ls -la","description":"list"}' }, bare)).toBe('ls -la')
  })
  it('pretty-prints other inputs and keeps a cut one as sent', () => {
    expect(toolInput({ 'attributes.tool_input': '{"a":1,"b":2}' }, bare)).toBe('{\n  "a": 1,\n  "b": 2\n}')
    expect(toolInput({ 'attributes.tool_input': '{"a":"lo…[90 chars]' }, bare)).toBe('{"a":"lo…[90 chars]')
  })
  it('falls back to the span when no log matched', () => {
    expect(toolInput(undefined, bare)).toBe('ls')
  })
})

describe('groupSessions', () => {
  it('groups turns by session, latest activity first', () => {
    const s = groupSessions([
      { traceId: 'a', startNs: 1, durationNs: 1, conversation: 's1' },
      { traceId: 'b', startNs: 5, durationNs: 1, conversation: 's2' },
      { traceId: 'c', startNs: 9, durationNs: 1, conversation: 's1' },
    ])
    expect(s.map((x) => [x.id, x.turns.length])).toEqual([
      ['s1', 2],
      ['s2', 1],
    ])
  })
})
