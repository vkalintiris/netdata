// The LiteLLM proxy as Netdata sees it: the charts go.d's stock LiteLLM profile builds from the proxy's Prometheus
// metrics (contexts `prometheus.litellm.*`), and the spans the proxy sends over OTLP, one `litellm_request` span per
// call it forwards, which the plugin marks `netdata.genai.call = gateway`.
import { absolute } from './page-state'
import { K, postTraces, type Condition, type ExploreAnswer } from './traces'
import type { TimeWindow } from './agent'

const P = 'prometheus.litellm.'

export const C = {
  requests: `${P}gateway.client_traffic.request_outcomes`,
  failureCauses: `${P}gateway.client_traffic.failure_causes`,
  inFlight: `${P}gateway.client_traffic.in_flight_requests`,
  requestLatency: `${P}gateway.request_latency.distribution`,
  providerLatency: `${P}provider_api.latency_distribution`,
  overhead: `${P}gateway.processing_overhead.distribution`,
  ttft: `${P}provider_api.time_to_first_token_distribution`,
  spendByModel: `${P}usage.requested_model.spend`,
  spendByTeam: `${P}usage.team.spend`,
  spendByKey: `${P}usage.api_key.spend`,
  tokensByModel: `${P}usage.requested_model.request_token_throughput`,
  tokensByTeam: `${P}usage.team.request_token_throughput`,
  teamRemaining: `${P}governance.teams.remaining_budget`,
  teamLimit: `${P}governance.teams.budget_limit`,
  deploymentState: `${P}routing.deployment_health.state`,
  deploymentOutcomes: `${P}routing.deployment_requests.outcomes`,
  cooldowns: `${P}routing.deployment_health.cooldowns`,
  fallbacks: `${P}routing.fallbacks.outcomes`,
}

// Labels the profile keeps on its charts.
export const L = {
  statusClass: 'status_class',
  exceptionClass: 'exception_class',
  model: 'model',
  requestedModel: 'requested_model',
  team: 'team_alias',
  key: 'api_key_alias',
}

// Fields of the proxy's call spans.
export const S = {
  team: 'attributes.metadata.user_api_key_team_alias',
  key: 'attributes.metadata.user_api_key_alias',
  cost: 'attributes.gen_ai.cost.total_cost',
  userAgent: 'attributes.metadata.user_agent',
  scope: 'scope.name',
}

// What a click on a ranking narrows the request list to, kept in the URL apart from the OTel GenAI filters.
export const NARROW = { team: 'gw_team', key: 'gw_key', model: 'gw_model' } as const
export type Narrow = Partial<Record<keyof typeof NARROW, string>>

export function requestConditions(narrow: Narrow): Condition[] {
  const out: Condition[] = [
    { key: K.call, op: '=', values: ['gateway'] },
    { key: S.scope, op: '=', values: ['litellm'] },
  ]
  if (narrow.team) out.push({ key: S.team, op: '=', values: [narrow.team] })
  if (narrow.key) out.push({ key: S.key, op: '=', values: [narrow.key] })
  if (narrow.model) out.push({ key: K.requestModel, op: '=', values: [narrow.model] })
  return out
}

const REQUEST_COLUMNS = [S.team, S.key, K.requestModel, K.outcome, K.inputTokens, K.outputTokens, S.cost, K.errorType]

export function exploreRequests(
  w: TimeWindow,
  narrow: Narrow,
  anchor: string | undefined,
  signal?: AbortSignal,
): Promise<ExploreAnswer> {
  const rows = { limit: 100, columns: REQUEST_COLUMNS }
  const where = requestConditions(narrow)
  const { after, before } = absolute(w)
  const body = anchor
    ? { explore: { conditions: where, anchor, rows } }
    : { explore: { after, before, conditions: where, rows } }
  return postTraces<ExploreAnswer>(body, signal)
}

// LiteLLM's deployment state gauge: 0 healthy, 1 partial outage, 2 complete outage.
export function deploymentState(v: number | undefined): { text: string; tone?: 'error' } {
  if (v == null) return { text: 'no reading' }
  if (v >= 2) return { text: 'outage', tone: 'error' }
  if (v >= 1) return { text: 'partial outage', tone: 'error' }
  return { text: 'healthy' }
}
