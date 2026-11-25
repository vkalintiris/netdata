// agentgateway as Netdata sees it: go.d's generic charts of its Prometheus metrics (contexts
// `prometheus.agentgateway.<metric>`, one per metric, every label kept), its OTLP access logs (one record per request,
// rejected ones included) and its spans, which the plugin marks `netdata.genai.call = gateway`.
const P = 'prometheus.agentgateway.agentgateway_'

export const C = {
  requests: `${P}requests_total`,
  cost: `${P}gen_ai_client_cost_usd_total`,
  tokens: `${P}gen_ai_client_token_usage_sum`,
  duration: `${P}gen_ai_server_request_duration`,
  ttft: `${P}gen_ai_server_time_to_first_token`,
  upstream: `${P}upstream_call_duration_seconds`,
  catalog: `${P}cost_catalog_lookups_total`,
  mcp: `${P}mcp_requests_total`,
}

// Labels on its metrics; `team` is the lab's own, added from the key's metadata.
export const L = {
  status: 'status',
  reason: 'reason',
  model: 'gen_ai_request_model',
  tokenType: 'gen_ai_token_type',
  team: 'team',
  catalogStatus: 'status',
  subtype: 'subtype',
  mcpServer: 'server',
  mcpMethod: 'method',
  mcpResource: 'resource',
}

// Fields of its access log records.
export const F = {
  service: 'resource.attributes.service.name',
  status: 'attributes.http.response.status_code',
  method: 'attributes.http.request.method',
  path: 'attributes.url.path',
  team: 'attributes.team',
  model: 'attributes.gen_ai.request.model',
  inputTokens: 'attributes.gen_ai.usage.input_tokens',
  outputTokens: 'attributes.gen_ai.usage.output_tokens',
  cost: 'attributes.agw.ai.usage.cost.total',
  duration: 'attributes.duration',
  client: 'attributes.client.address',
  traceId: 'attributes.trace.id',
  mcpTarget: 'attributes.mcp.target',
  mcpMethod: 'attributes.mcp.method.name',
  tool: 'attributes.gen_ai.tool.name',
  cursor: 'cursor',
  timestamp: 'timestamp',
}

export const NARROW = { team: 'gw_team', model: 'gw_model', status: 'gw_status' } as const
export type Narrow = Partial<Record<keyof typeof NARROW, string>>

// The access log records of the lab's agentgateway, narrowed by what was clicked.
export function logSelections(narrow: Narrow): Record<string, string[]> {
  const out: Record<string, string[]> = { [F.service]: ['agentgateway'] }
  if (narrow.team) out[F.team] = [narrow.team]
  if (narrow.model) out[F.model] = [narrow.model]
  if (narrow.status) out[F.status] = [narrow.status]
  return out
}

// agentgateway writes durations as text with a unit (`1880ms`, `2.5s`); seconds, or null when unreadable.
export function durationSeconds(text: string | undefined): number | null {
  const m = text?.match(/^([\d.]+)(ns|µs|us|ms|s|m)$/)
  if (!m) return null
  const value = Number(m[1])
  if (m[2] === 'm') return value * 60
  // Divided, not multiplied by a fraction: 1880 / 1000 is exactly 1.88.
  const per: Record<string, number> = { ns: 1e9, µs: 1e6, us: 1e6, ms: 1e3, s: 1 }
  return value / per[m[2]]
}

// Status codes as classes, for the requests chart.
export function statusClass(code: string): string {
  return /^[1-5]\d\d$/.test(code) ? `${code[0]}xx` : 'other'
}
