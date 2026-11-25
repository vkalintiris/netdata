import { describe, expect, it } from 'vitest'
import { conversationOf, conversationOfSpan, invalidJsonNotice, invalidToolsNotice } from './conversation'
import type { Conversation } from './conversation'

const semconvInput = JSON.stringify([
  { role: 'system', parts: [{ content: 'You are a terse assistant.', type: 'text' }] },
  { role: 'user', parts: [{ content: 'What is the weather in Faketown?', type: 'text' }] },
  {
    role: 'assistant',
    parts: [
      { arguments: { city: 'Faketown' }, name: 'get_weather', id: 'call_9364', type: 'tool_call' },
      { content: '', type: 'text' },
    ],
  },
  {
    role: 'tool',
    parts: [{ response: 'sunny, 21C', id: 'call_9364', type: 'tool_call_response' }],
  },
])

const semconvOutput = JSON.stringify([
  {
    role: 'assistant',
    parts: [
      {
        content: 'The weather in Faketown is sunny with a temperature of 21°C.',
        type: 'text',
      },
    ],
    finish_reason: 'stop',
  },
])

const toolDefinitions = JSON.stringify([
  {
    type: 'function',
    name: 'get_weather',
    description: 'Get the weather for a city',
    parameters: {
      type: 'object',
      properties: { city: { type: 'string' } },
      required: ['city'],
    },
  },
])

const weatherToolCall = (id: string, args: string) => ({
  kind: 'toolCall',
  name: 'get_weather',
  arguments: args,
  id,
})

// The source tests assume a non-null result and would throw on null.
const must = (conversation: Conversation | null): Conversation => {
  if (conversation === null) throw new Error('expected a conversation')
  return conversation
}

describe("conversationOf with the conventions' JSON attributes", () => {
  it('normalizes the input, output and tool definitions, dropping empty text parts', () => {
    expect(
      conversationOf([
        ['name', 'chat gpt-4o'],
        ['attributes.gen_ai.input.messages', semconvInput],
        ['attributes.gen_ai.output.messages', semconvOutput],
        ['attributes.gen_ai.tool.definitions', toolDefinitions],
      ]),
    ).toEqual({
      messages: [
        {
          section: 'input',
          role: 'system',
          parts: [{ kind: 'text', text: 'You are a terse assistant.' }],
        },
        {
          section: 'input',
          role: 'user',
          parts: [{ kind: 'text', text: 'What is the weather in Faketown?' }],
        },
        {
          section: 'input',
          role: 'assistant',
          parts: [weatherToolCall('call_9364', '{\n  "city": "Faketown"\n}')],
        },
        {
          section: 'input',
          role: 'tool',
          parts: [{ kind: 'toolResult', text: 'sunny, 21C', id: 'call_9364' }],
        },
        {
          section: 'output',
          role: 'assistant',
          finishReason: 'stop',
          parts: [
            {
              kind: 'text',
              text: 'The weather in Faketown is sunny with a temperature of 21°C.',
            },
          ],
        },
      ],
      tools: ['get_weather'],
      notices: [],
    })
  })

  it('reads the Anthropic variant with a null name and the tool result sent as a user message', () => {
    const input = JSON.stringify([
      { role: 'user', name: null, parts: [{ content: 'What is the weather?', type: 'text' }] },
      {
        role: 'assistant',
        name: null,
        parts: [
          {
            arguments: { city: 'Faketown' },
            name: 'get_weather',
            id: 'toolu_1',
            type: 'tool_call',
          },
        ],
      },
      {
        role: 'user',
        name: null,
        parts: [{ response: 'sunny, 21C', id: 'toolu_1', type: 'tool_call_response' }],
      },
    ])

    expect(must(conversationOf([['attributes.gen_ai.input.messages', input]])).messages).toEqual([
      { section: 'input', role: 'user', parts: [{ kind: 'text', text: 'What is the weather?' }] },
      {
        section: 'input',
        role: 'assistant',
        parts: [weatherToolCall('toolu_1', '{\n  "city": "Faketown"\n}')],
      },
      {
        section: 'input',
        role: 'user',
        parts: [{ kind: 'toolResult', text: 'sunny, 21C', id: 'toolu_1' }],
      },
    ])
  })

  it('shows the system instructions as a system message first', () => {
    const { messages } = must(
      conversationOf([
        ['attributes.gen_ai.input.messages', JSON.stringify([{ role: 'user', parts: ['Hi'] }])],
        ['attributes.gen_ai.system_instructions', JSON.stringify([{ type: 'text', content: 'Be brief.' }])],
      ]),
    )
    expect(messages).toEqual([
      { section: 'input', role: 'system', parts: [{ kind: 'text', text: 'Be brief.' }] },
      { section: 'input', role: 'user', parts: [{ kind: 'text', text: 'Hi' }] },
    ])
  })

  it('shows an unknown part type with its content or JSON', () => {
    const input = JSON.stringify([
      {
        role: 'user',
        parts: [
          { type: 'blob', modality: 'image', mime_type: 'image/png' },
        ],
      },
    ])
    expect(must(conversationOf([['attributes.gen_ai.input.messages', input]])).messages[0].parts).toEqual([
      {
        kind: 'other',
        type: 'blob',
        text: '{\n  "modality": "image",\n  "mime_type": "image/png"\n}',
      },
    ])
  })

  it('shows the model\'s reasoning as reasoning, under each name producers give it', () => {
    const input = JSON.stringify([
      {
        role: 'assistant',
        parts: [
          { type: 'reasoning', content: 'first' },
          { type: 'thinking', content: 'second' },
          { type: 'reasoningContent', content: { reasoningText: { text: 'third' } } },
          { type: 'reasoning', content: '' },
        ],
      },
    ])
    expect(must(conversationOf([['attributes.gen_ai.input.messages', input]])).messages[0].parts).toEqual([
      { kind: 'reasoning', text: 'first' },
      { kind: 'reasoning', text: 'second' },
      { kind: 'reasoning', text: 'third' },
    ])
  })

  it('reads the messages a span carries in its details events', () => {
    const event = (key: string, value: string) => ({
      name: 'gen_ai.client.inference.operation.details',
      attributes: [[`events.attributes.${key}`, value]],
    })
    const span = {
      fields: [['name', 'chat']],
      events: [
        event('gen_ai.system_instructions', JSON.stringify([{ type: 'text', content: 'Be brief.' }])),
        event('gen_ai.input.messages', JSON.stringify([{ role: 'user', parts: [{ type: 'text', content: 'Hi' }] }])),
        event('gen_ai.output.messages', JSON.stringify([{ role: 'assistant', parts: [{ type: 'text', content: 'Hello' }] }])),
        { name: 'exception', attributes: [['events.attributes.gen_ai.input.messages', 'ignored']] },
      ],
    }
    expect(must(conversationOfSpan(span)).messages.map((m) => [m.section, m.role, m.parts])).toEqual([
      ['input', 'system', [{ kind: 'text', text: 'Be brief.' }]],
      ['input', 'user', [{ kind: 'text', text: 'Hi' }]],
      ['output', 'assistant', [{ kind: 'text', text: 'Hello' }]],
    ])
    expect(conversationOfSpan({ fields: [['name', 'chat']], events: [] })).toBeNull()
  })

  it('shows a notice and the raw text when a value was cut', () => {
    const cut = semconvInput.slice(0, 120)
    expect(
      conversationOf([
        ['attributes.gen_ai.input.messages', cut],
        ['attributes.gen_ai.output.messages', semconvOutput],
        ['attributes.gen_ai.tool.definitions', toolDefinitions.slice(0, 40)],
      ]),
    ).toEqual({
      messages: [
        {
          section: 'input',
          role: null,
          notice: invalidJsonNotice,
          parts: [{ kind: 'text', text: cut }],
        },
        expect.objectContaining({ section: 'output', role: 'assistant' }),
      ],
      tools: [],
      notices: [invalidToolsNotice],
    })
  })

  it('degrades missing roles, plain text and unknown shapes to text', () => {
    const input = JSON.stringify([
      { parts: [{ type: 'text', content: 'no role' }] },
      'a bare string',
      { role: 'user', content: 'legacy content' },
      { role: 'user', something: 1 },
    ])
    expect(
      must(
        conversationOf([
          ['attributes.gen_ai.input.messages', input],
          ['attributes.gen_ai.output.messages', 'just text'],
        ]),
      ).messages,
    ).toEqual([
      { section: 'input', role: null, parts: [{ kind: 'text', text: 'no role' }] },
      { section: 'input', role: null, parts: [{ kind: 'text', text: 'a bare string' }] },
      { section: 'input', role: 'user', parts: [{ kind: 'text', text: 'legacy content' }] },
      {
        section: 'input',
        role: 'user',
        parts: [{ kind: 'text', text: '{\n  "role": "user",\n  "something": 1\n}' }],
      },
      { section: 'output', role: null, parts: [{ kind: 'text', text: 'just text' }] },
    ])
  })

  it.each<[string, unknown]>([
    ['no fields', undefined],
    ['unrelated fields', [['attributes.http.route', '/checkout']]],
    ['only empty messages', [['attributes.gen_ai.input.messages', '[]']]],
    ['a null message value', [['attributes.gen_ai.input.messages', null]]],
    ['only tool definitions', [['attributes.gen_ai.tool.definitions', toolDefinitions]]],
    ['malformed entries', [null, 'x', [], [42]]],
  ])('returns null for %s', (_, fields) => {
    expect(conversationOf(fields)).toBeNull()
  })
})

describe('conversationOf with indexed attributes', () => {
  const llm = (section: string, index: number, field: string, value: unknown): [string, unknown] => [
    `attributes.llm.${section}_messages.${index}.message.${field}`,
    value,
  ]

  it('normalizes OpenInference messages and tool names', () => {
    expect(
      conversationOf([
        llm('output', 0, 'role', 'assistant'),
        llm('output', 0, 'content', 'The weather in Faketown is sunny, 21°C.'),
        llm('input', 0, 'role', 'system'),
        llm('input', 0, 'content', 'You are a terse assistant.'),
        llm('input', 1, 'role', 'user'),
        llm('input', 1, 'content', 'What is the weather in Faketown?'),
        llm('input', 2, 'role', 'assistant'),
        llm('input', 2, 'tool_calls.0.tool_call.id', 'call_1031'),
        llm('input', 2, 'tool_calls.0.tool_call.function.name', 'get_weather'),
        llm('input', 2, 'tool_calls.0.tool_call.function.arguments', '{"city": "Faketown"}'),
        llm('input', 3, 'role', 'tool'),
        llm('input', 3, 'tool_call_id', 'call_1031'),
        llm('input', 3, 'content', 'sunny, 21C'),
        [
          'attributes.llm.tools.0.tool.json_schema',
          '{"type": "function", "function": {"name": "get_weather", "parameters": {}}}',
        ],
        ['attributes.llm.tools.1.tool.json_schema', '{"type": "function", "function": {"na'],
      ]),
    ).toEqual({
      messages: [
        {
          section: 'input',
          role: 'system',
          parts: [{ kind: 'text', text: 'You are a terse assistant.' }],
        },
        {
          section: 'input',
          role: 'user',
          parts: [{ kind: 'text', text: 'What is the weather in Faketown?' }],
        },
        {
          section: 'input',
          role: 'assistant',
          parts: [weatherToolCall('call_1031', '{"city": "Faketown"}')],
        },
        {
          section: 'input',
          role: 'tool',
          parts: [{ kind: 'toolResult', text: 'sunny, 21C', id: 'call_1031' }],
        },
        {
          section: 'output',
          role: 'assistant',
          parts: [{ kind: 'text', text: 'The weather in Faketown is sunny, 21°C.' }],
        },
      ],
      tools: ['get_weather'],
      notices: [],
    })
  })

  it('orders messages and tool calls numerically, so 10 comes after 9', () => {
    const fields: [string, unknown][] = []
    for (const index of [10, 2, 9, 1, 0]) fields.push(llm('input', index, 'content', `m${index}`))
    fields.push(llm('input', 11, 'tool_calls.10.tool_call.function.name', 'late'))
    fields.push(llm('input', 11, 'tool_calls.9.tool_call.function.name', 'early'))

    const { messages } = must(conversationOf(fields))
    // The source reads `text` and `name` off the untyped part; narrow by kind here.
    const first = (message: (typeof messages)[number]) => {
      const part = message.parts[0]
      return part.kind === 'toolCall' ? part.name : 'text' in part ? part.text : undefined
    }
    expect(messages.map(first)).toEqual(['m0', 'm1', 'm2', 'm9', 'm10', 'early'])
    expect(messages[5].parts.map((part) => (part.kind === 'toolCall' ? part.name : undefined))).toEqual([
      'early',
      'late',
    ])
    expect(messages[0].role).toBeNull()
  })

  it('keeps unknown indexed fields as other parts and drops empty content', () => {
    expect(
      must(
        conversationOf([
          llm('input', 0, 'role', 'user'),
          llm('input', 0, 'content', ''),
          llm('input', 0, 'contents.1.message_content.text', 'second'),
          llm('input', 0, 'contents.0.message_content.type', 'text'),
          llm('input', 0, 'contents.0.message_content.text', 'first'),
          llm('input', 0, 'contents.2.message_content.image.image.url', 'https://img'),
        ]),
      ).messages,
    ).toEqual([
      {
        section: 'input',
        role: 'user',
        parts: [
          { kind: 'text', text: 'first' },
          { kind: 'text', text: 'second' },
          {
            kind: 'other',
            type: 'contents.2.message_content.image.image.url',
            text: 'https://img',
          },
        ],
      },
    ])
  })

  it('reads the older OpenLLMetry prompt and completion attributes', () => {
    expect(
      conversationOf([
        ['attributes.gen_ai.completion.0.role', 'assistant'],
        ['attributes.gen_ai.completion.0.content', 'Sunny.'],
        ['attributes.gen_ai.completion.0.finish_reason', 'stop'],
        ['attributes.gen_ai.prompt.0.role', 'user'],
        ['attributes.gen_ai.prompt.0.content', 'Weather?'],
        ['attributes.gen_ai.prompt.1.role', 'assistant'],
        ['attributes.gen_ai.prompt.1.tool_calls.0.name', 'get_weather'],
        ['attributes.gen_ai.prompt.1.tool_calls.0.arguments', '{"city":"Faketown"}'],
        ['attributes.gen_ai.prompt.1.tool_calls.0.id', 'call_1'],
      ]),
    ).toEqual({
      messages: [
        { section: 'input', role: 'user', parts: [{ kind: 'text', text: 'Weather?' }] },
        {
          section: 'input',
          role: 'assistant',
          parts: [weatherToolCall('call_1', '{"city":"Faketown"}')],
        },
        {
          section: 'output',
          role: 'assistant',
          finishReason: 'stop',
          parts: [{ kind: 'text', text: 'Sunny.' }],
        },
      ],
      tools: [],
      notices: [],
    })
  })

  it("prefers the conventions' JSON attributes when both shapes are present", () => {
    const { messages } = must(
      conversationOf([
        llm('input', 0, 'content', 'indexed'),
        ['attributes.gen_ai.output.messages', semconvOutput],
      ]),
    )
    expect(messages).toEqual([expect.objectContaining({ section: 'output', role: 'assistant' })])
  })
})
