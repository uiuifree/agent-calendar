import { describe, expect, it } from 'vitest'
import { formatRoute, parseRoute } from './route.js'

describe('URL と表示', () => {
  it('URL を読む', () => {
    expect(parseRoute('/')).toEqual({ view: 'calendar', owner: null, session: null, tab: 'summary', full: false })
    expect(parseRoute('/', '?session=abc')).toEqual({ view: 'calendar', owner: null, session: 'abc', tab: 'summary', full: false })
    expect(parseRoute('/', '?session=abc&tab=changes&full=1')).toMatchObject({ tab: 'changes', full: true })
    // セッションが無ければタブも全画面も無い。知らないタブは概要
    expect(parseRoute('/', '?tab=changes&full=1')).toMatchObject({ tab: 'summary', full: false })
    expect(parseRoute('/', '?session=a&tab=x')).toMatchObject({ tab: 'summary' })
    expect(parseRoute('/stats').view).toBe('stats')
    expect(parseRoute('/schedules').view).toBe('plans')
    expect(parseRoute('/repos/my-org')).toMatchObject({ view: 'repos', owner: 'my-org', session: null })
    expect(parseRoute('/repos')).toMatchObject({ view: 'repos', owner: null, session: null })
    // 知らない URL（前の版の /week/… など）はカレンダー
    expect(parseRoute('/week/2026-10-05').view).toBe('calendar')
    expect(parseRoute('/stats/x').owner).toBe(null)
  })

  it('URL を作る（カレンダーは日・週・月とも "/"）', () => {
    for (const view of ['day', 'week', 'month']) expect(formatRoute({ view })).toBe('/')
    expect(formatRoute({ view: 'week', session: 'a b' })).toBe('/?session=a%20b')
    expect(formatRoute({ view: 'week', session: 's', tab: 'changes', full: true })).toBe('/?session=s&tab=changes&full=1')
    expect(formatRoute({ view: 'week', tab: 'changes', full: true })).toBe('/')
    expect(formatRoute({ view: 'stats' })).toBe('/stats')
    expect(formatRoute({ view: 'plans' })).toBe('/schedules')
    expect(formatRoute({ view: 'repos', owner: 'uiuifree' })).toBe('/repos/uiuifree')
    expect(formatRoute({ view: 'repos', owner: null })).toBe('/repos')
  })

  it('作った URL を読むと同じ画面に戻る', () => {
    const state = { view: 'repos', owner: 'uiuifree', session: 's1', tab: 'conversation', full: true }
    const url = new URL(formatRoute(state), 'http://x')
    expect(parseRoute(url.pathname, url.search)).toEqual(state)
  })
})
