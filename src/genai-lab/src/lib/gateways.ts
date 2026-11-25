// The gateways that have a page, by the name a trace link carries back (`from=<name>`).
export const GATEWAY_PAGES: Record<string, { path: string; label: string }> = {
  litellm: { path: '/gateways/litellm', label: 'LiteLLM' },
  agentgateway: { path: '/gateways/agentgateway', label: 'agentgateway' },
}
