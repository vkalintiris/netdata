const genaiInputMessagesKey = 'attributes.gen_ai.input.messages'
const genaiOutputMessagesKey = 'attributes.gen_ai.output.messages'
const genaiSystemInstructionsKey = 'attributes.gen_ai.system_instructions'
const genaiToolDefinitionsKey = 'attributes.gen_ai.tool.definitions'

export const invalidJsonNotice = 'The content was cut or is not valid JSON. Showing the raw text.'
export const invalidToolsNotice = 'The tool definitions were cut or are not valid JSON.'

export type Part =
  | { kind: 'text'; text: string }
  | { kind: 'toolCall'; name: string | null; arguments: string; id: string | null }
  | { kind: 'toolResult'; text: string; id: string | null }
  | { kind: 'reasoning'; text: string }
  | { kind: 'other'; type: string; text: string }

export type Message = {
  section: 'input' | 'output'
  role: string | null
  parts: Part[]
  finishReason?: string
  notice?: string
}

export type Conversation = {
  messages: Message[]
  tools: string[]
  notices: string[]
}

type Section = Message['section']
type Parsed = { value: unknown } | { text: string } | { invalid: true }
type IndexedEntry = { section: Section; index: number; fields: Map<string, unknown> }

const indexedSources = [
  /^attributes\.llm\.(input|output)_messages\.(\d+)\.message\.(.+)$/,
  /^attributes\.gen_ai\.(prompt|completion)\.(\d+)\.(.+)$/,
]
const indexedSections: Record<string, Section> = {
  input: 'input',
  output: 'output',
  prompt: 'input',
  completion: 'output',
}
const indexedToolPattern = /^attributes\.llm\.tools\.(\d+)\.tool\.json_schema$/
const toolCallPattern = /^tool_calls\.(\d+)\.(?:tool_call\.)?(?:function\.)?(id|name|arguments)$/
const contentItemPattern = /^contents\.(\d+)\.message_content\.(type|text)$/
const ignoredIndexedFields = new Set(['role', 'tool_call_id', 'name', 'finish_reason'])
const looksLikeJson = /^\s*[[{]/

const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === 'object' && !Array.isArray(value)

const toArray = (value: unknown): unknown[] => (Array.isArray(value) ? value : [value])

const nonEmptyString = (value: unknown): string | null => (typeof value === 'string' && value ? value : null)

const asText = (value: unknown): string => {
  if (value === null || value === undefined) return ''
  if (typeof value === 'string') return value
  if (typeof value === 'object') return JSON.stringify(value, null, 2) ?? ''
  return String(value)
}

const hasText = (text: string): boolean => text.trim() !== ''

const parseValue = (raw: unknown): Parsed => {
  if (typeof raw !== 'string') return { value: raw }
  if (!looksLikeJson.test(raw)) return { text: raw }
  try {
    return { value: JSON.parse(raw) }
  } catch {
    return { invalid: true }
  }
}

const textPart = (text: string): Part => ({ kind: 'text', text })

const textPartOf = (value: unknown): Part | null => {
  const text = asText(value)
  return hasText(text) ? textPart(text) : null
}

const isPart = (part: Part | null): part is Part => part !== null

const isMessage = (message: Message | null): message is Message => message !== null

// The model's reasoning, under the names instrumentations give the part.
const reasoningTypes = new Set(['reasoning', 'thinking', 'reasoningContent'])

// Strands nests the text: {"reasoningText": {"text": ...}}.
const reasoningTextOf = (content: unknown): unknown => {
  if (isObject(content) && isObject(content.reasoningText) && typeof content.reasoningText.text === 'string') {
    return content.reasoningText.text
  }
  return content
}

const normalizePart = (part: unknown): Part | null => {
  if (!isObject(part)) return textPartOf(part)

  if (part.type === 'text' || (part.type === undefined && typeof part.content === 'string')) {
    return textPartOf(part.content)
  }
  if (part.type === 'tool_call') {
    return {
      kind: 'toolCall',
      name: nonEmptyString(part.name),
      arguments: asText(part.arguments),
      id: nonEmptyString(part.id),
    }
  }
  if (part.type === 'tool_call_response') {
    return { kind: 'toolResult', text: asText(part.response), id: nonEmptyString(part.id) }
  }

  if (typeof part.type === 'string' && reasoningTypes.has(part.type)) {
    const text = asText(reasoningTextOf(part.content))
    return hasText(text) ? { kind: 'reasoning', text } : null
  }

  const { type, ...rest } = part
  return {
    kind: 'other',
    type: nonEmptyString(type) || 'unknown',
    text: asText('content' in rest ? rest.content : rest),
  }
}

const messageOf = (
  section: Section,
  role: string | null,
  parts: Part[],
  extra: { finishReason?: string } = {},
): Message | null => (parts.length ? { section, role, parts, ...extra } : null)

const normalizeMessage = (message: unknown, section: Section): Message | null => {
  if (!isObject(message)) return messageOf(section, null, [textPartOf(message)].filter(isPart))

  let parts: Part[]
  if (Array.isArray(message.parts)) parts = message.parts.map(normalizePart).filter(isPart)
  else if ('content' in message) parts = [textPartOf(message.content)].filter(isPart)
  else parts = [textPart(asText(message))]

  const finishReason = nonEmptyString(message.finish_reason)
  return messageOf(section, nonEmptyString(message.role), parts, finishReason ? { finishReason } : {})
}

const invalidMessage = (section: Section, role: string | null, raw: unknown): Message => ({
  section,
  role,
  notice: invalidJsonNotice,
  parts: [textPart(raw as string)],
})

const jsonMessages = (raw: unknown, section: Section): (Message | null)[] => {
  if (raw === undefined) return []

  const parsed = parseValue(raw)
  if ('invalid' in parsed) return [invalidMessage(section, null, raw)]
  if ('text' in parsed) return [messageOf(section, null, [textPartOf(parsed.text)].filter(isPart))]

  return toArray(parsed.value).map((message) => normalizeMessage(message, section))
}

const systemMessage = (raw: unknown): Message | null => {
  if (raw === undefined) return null

  const parsed = parseValue(raw)
  if ('invalid' in parsed) return invalidMessage('input', 'system', raw)

  const items = 'text' in parsed ? [parsed.text] : toArray(parsed.value)
  return messageOf('input', 'system', items.map(normalizePart).filter(isPart))
}

const semconvMessages = (map: Map<unknown, unknown>): Message[] =>
  [
    systemMessage(map.get(genaiSystemInstructionsKey)),
    ...jsonMessages(map.get(genaiInputMessagesKey), 'input'),
    ...jsonMessages(map.get(genaiOutputMessagesKey), 'output'),
  ].filter(isMessage)

const entryOf = <K, V>(entries: Map<K, V>, index: K, create: () => V): V => {
  let entry = entries.get(index)
  if (entry === undefined) {
    entry = create()
    entries.set(index, entry)
  }
  return entry
}

const byIndex = <V>(entries: Map<number, V>): V[] =>
  [...entries.entries()].sort(([a], [b]) => a - b).map(([, v]) => v)

const indexedMessage = ({ section, fields }: IndexedEntry): Message | null => {
  const role = nonEmptyString(fields.get('role'))
  const toolCallId = nonEmptyString(fields.get('tool_call_id'))
  const isToolResult = role === 'tool' || !!toolCallId
  const contents = new Map<number, Record<string, unknown>>()
  const toolCalls = new Map<number, Record<string, unknown>>()
  const others: Part[] = []

  for (const [field, value] of fields) {
    if (ignoredIndexedFields.has(field) || field === 'content') continue

    const toolCall = field.match(toolCallPattern)
    if (toolCall) {
      entryOf(toolCalls, Number(toolCall[1]), (): Record<string, unknown> => ({}))[toolCall[2]] = value
      continue
    }

    const contentItem = field.match(contentItemPattern)
    if (contentItem) {
      entryOf(contents, Number(contentItem[1]), (): Record<string, unknown> => ({}))[contentItem[2]] = value
      continue
    }

    others.push({ kind: 'other', type: field, text: asText(value) })
  }

  const parts: Part[] = []
  if (fields.has('content')) {
    const text = asText(fields.get('content'))
    if (isToolResult) parts.push({ kind: 'toolResult', text, id: toolCallId })
    else if (hasText(text)) parts.push(textPart(text))
  }

  for (const item of byIndex(contents)) {
    if (item.text !== undefined) {
      const part = textPartOf(item.text)
      if (part) parts.push(part)
    }
  }

  for (const call of byIndex(toolCalls)) {
    parts.push({
      kind: 'toolCall',
      name: nonEmptyString(call.name),
      arguments: asText(call.arguments),
      id: nonEmptyString(call.id),
    })
  }

  parts.push(...others)

  const finishReason = nonEmptyString(fields.get('finish_reason'))
  return messageOf(section, role, parts, finishReason ? { finishReason } : {})
}

const indexedMessages = (pairs: unknown[][]): Message[] => {
  for (const pattern of indexedSources) {
    const entries = new Map<string, IndexedEntry>()

    for (const [key, value] of pairs) {
      const match = String(key).match(pattern)
      if (!match) continue

      const section = indexedSections[match[1]]
      const index = Number(match[2])
      const entry = entryOf(entries, `${section}:${index}`, () => ({
        section,
        index,
        fields: new Map<string, unknown>(),
      }))
      if (!entry.fields.has(match[3])) entry.fields.set(match[3], value)
    }

    if (!entries.size) continue

    return [...entries.values()]
      .sort((a, b) => (a.section === b.section ? a.index - b.index : a.section === 'input' ? -1 : 1))
      .map(indexedMessage)
      .filter(isMessage)
  }

  return []
}

const toolNameOf = (definition: unknown): string | null => {
  if (!isObject(definition)) return null
  const fn = definition.function
  return nonEmptyString(definition.name) || nonEmptyString(isObject(fn) ? fn.name : undefined)
}

const isString = (value: string | null): value is string => value !== null

const toolsOf = (
  map: Map<unknown, unknown>,
  pairs: unknown[][],
): { tools: string[]; notices: string[] } => {
  if (map.has(genaiToolDefinitionsKey)) {
    const parsed = parseValue(map.get(genaiToolDefinitionsKey))
    if ('invalid' in parsed) return { tools: [], notices: [invalidToolsNotice] }
    if ('text' in parsed) return { tools: [], notices: [] }
    return { tools: toArray(parsed.value).map(toolNameOf).filter(isString), notices: [] }
  }

  const schemas = new Map<number, unknown>()
  for (const [key, value] of pairs) {
    const match = String(key).match(indexedToolPattern)
    if (match && !schemas.has(Number(match[1]))) schemas.set(Number(match[1]), value)
  }

  const tools = byIndex(schemas)
    .map((schema) => {
      const parsed = parseValue(schema)
      return 'value' in parsed ? toolNameOf(parsed.value) : null
    })
    .filter(isString)
  return { tools, notices: [] }
}

const pairsOf = (fields: unknown): unknown[][] =>
  (Array.isArray(fields) ? fields : []).filter(
    (entry): entry is unknown[] => Array.isArray(entry) && entry.length > 0,
  )

const firstValues = (pairs: unknown[][]): Map<unknown, unknown> => {
  const map = new Map<unknown, unknown>()
  for (const [key, value] of pairs) {
    if (!map.has(key)) map.set(key, value)
  }
  return map
}

// The conventions' event that carries a call's messages as its attributes, for producers that keep them off the
// span's own attributes.
const detailsEvent = 'gen_ai.client.inference.operation.details'
const eventAttributePrefix = 'events.attributes.'

// The conversation of a span: from its attributes, or from its details events when the attributes carry none.
export const conversationOfSpan = (span: {
  fields?: unknown
  events?: { name?: string; attributes?: unknown }[]
}): Conversation | null => {
  const pairs = pairsOf(span.fields)
  for (const event of span.events ?? []) {
    if (event.name !== detailsEvent) continue
    for (const [key, value] of pairsOf(event.attributes)) {
      if (typeof key === 'string' && key.startsWith(eventAttributePrefix)) {
        pairs.push([`attributes.${key.slice(eventAttributePrefix.length)}`, value])
      }
    }
  }
  return conversationOf(pairs)
}

export const conversationOf = (fields: [string, unknown][] | unknown): Conversation | null => {
  const pairs = pairsOf(fields)
  const map = firstValues(pairs)
  const hasSemconv = [genaiSystemInstructionsKey, genaiInputMessagesKey, genaiOutputMessagesKey].some((key) =>
    map.has(key),
  )
  const messages = hasSemconv ? semconvMessages(map) : indexedMessages(pairs)
  if (!messages.length) return null

  return { messages, ...toolsOf(map, pairs) }
}
