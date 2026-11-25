import { describe, expect, it } from 'vitest'
import { durationSeconds, logSelections, statusClass } from './agentgateway'

describe('agentgateway', () => {
  it('reads its duration text', () => {
    expect(['1880ms', '2.5s', '12µs', '1m', 'soon', undefined].map(durationSeconds)).toEqual([
      1.88, 2.5, 0.000012, 60, null, null,
    ])
  })

  it('asks for its access logs, narrowed by what was clicked', () => {
    expect(logSelections({ team: 'search', status: '401' })).toEqual({
      'resource.attributes.service.name': ['agentgateway'],
      'attributes.team': ['search'],
      'attributes.http.response.status_code': ['401'],
    })
  })

  it('groups status codes', () => {
    expect(['200', '401', '503', 'x'].map(statusClass)).toEqual(['2xx', '4xx', '5xx', 'other'])
  })
})
