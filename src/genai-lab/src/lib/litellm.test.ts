import { describe, expect, it } from 'vitest'
import { deploymentState, requestConditions } from './litellm'

describe('litellm', () => {
  it('asks for the proxy call spans, narrowed by what was clicked', () => {
    expect(requestConditions({ team: 'search', model: 'qwen' })).toEqual([
      { key: 'attributes.netdata.genai.call', op: '=', values: ['gateway'] },
      { key: 'scope.name', op: '=', values: ['litellm'] },
      { key: 'attributes.metadata.user_api_key_team_alias', op: '=', values: ['search'] },
      { key: 'attributes.netdata.genai.request_model', op: '=', values: ['qwen'] },
    ])
  })

  it('names the deployment states', () => {
    expect([0, 1, 2, undefined].map(deploymentState)).toEqual([
      { text: 'healthy' },
      { text: 'partial outage', tone: 'error' },
      { text: 'outage', tone: 'error' },
      { text: 'no reading' },
    ])
  })
})
