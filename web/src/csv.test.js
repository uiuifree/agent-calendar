import { expect, it } from 'vitest'
import { statsCsv } from './csv.js'

// CLI の stats --csv（stats.rs の table のテスト）と同じ形になること
it('案件 → リポジトリの行を CSV にする', () => {
  const stats = {
    projects: [
      { project: 'acme', repos: [{ repo: '/r/acme-api', secs: 1200, sessions: 1, usd: 20, unpriced: [] }] },
      { project: '', repos: [{ repo: '/r/private', secs: 600, sessions: 1, usd: 0, unpriced: ['local-model'] }] },
      { project: 'A, "B"', repos: [{ repo: '/r/x\ny', secs: 5400, sessions: 3, usd: 1.234, unpriced: [] }] },
    ],
  }
  expect(statsCsv(stats)).toBe(
    'project,repository,hours,sessions,usd\n' +
      'acme,/r/acme-api,0.3,1,20.00\n' +
      ',/r/private,0.2,1,0.00+\n' +
      '"A, ""B""","/r/x\ny",1.5,3,1.23\n',
  )
  expect(statsCsv({ projects: [] })).toBe('project,repository,hours,sessions,usd\n')
})
