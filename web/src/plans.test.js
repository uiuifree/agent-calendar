import { describe as group, expect, it } from 'vitest'
import { setLang } from './i18n.js'
import { ALL_DAYS, WEEKDAYS, WEEKEND, copyOf, dayLabel, describe, everyLabel } from './plans.js'

group('予定の文', () => {
  it('曜日', () => {
    setLang('ja')
    expect(dayLabel(ALL_DAYS)).toBe('毎日')
    expect(dayLabel(WEEKDAYS)).toBe('平日')
    expect(dayLabel(WEEKEND)).toBe('土日')
    expect(dayLabel(0b0010101)).toBe('月・水・金')
  })
  it('繰り返し', () => {
    setLang('ja')
    expect(describe({ kind: 'weekly', days: WEEKDAYS, minute: 8 * 60 + 5 })).toBe('平日 8:05')
    expect(describe({ kind: 'interval', days: ALL_DAYS, every_min: 120, from_hour: 7, to_hour: 22 })).toBe('毎日 7:00–22:00 2 時間ごと')
    expect(describe({ kind: 'interval', days: ALL_DAYS, every_min: 30, from_hour: 9, to_hour: 18 })).toContain('30 分ごと')
    expect(describe({ kind: 'once', at: new Date(2026, 9, 5, 9, 0).getTime() })).toContain('1 回だけ')
    expect(describe(null)).toBe('')
    setLang('en')
    expect(describe({ kind: 'weekly', days: ALL_DAYS, minute: 60 })).toBe('Every day 1:00')
  })
})

group('間隔の文', () => {
  it('60 で割り切れれば時間、それ以外は分', () => {
    setLang('ja')
    expect(everyLabel(120)).toBe('2 時間ごと')
    expect(everyLabel(30)).toBe('30 分ごと')
  })
})

group('予定のコピー', () => {
  const at = (s) => new Date(s).getTime()
  const plan = {
    id: 2, name: 'OSSチェック', dir: '/r', agent: 'claude', mode: 'read', worktree: false, continue_session: false,
    prompt: '確認して', enabled: false, next_at: null, last_session_id: 's1', runs: [{ id: 1 }],
    repeat: { kind: 'once', at: at('2026-10-04T13:00:00') },
  }
  it('id・実行の記録を持たず、動く状態で始まる', () => {
    setLang('ja')
    const c = copyOf(plan, at('2026-10-04T15:00:00'))
    expect(c.id).toBeUndefined()
    expect(c.runs).toBeUndefined()
    expect(c.last_session_id).toBeUndefined()
    expect(c.name).toBe('OSSチェック（コピー）')
    expect(c.enabled).toBe(true)
    expect(c.prompt).toBe('確認して')
  })
  it('過ぎた 1 回だけの予定は翌日の同じ時刻に。先の時刻と繰り返しはそのまま', () => {
    expect(copyOf(plan, at('2026-10-04T15:00:00')).repeat.at).toBe(at('2026-10-05T13:00:00'))
    expect(copyOf(plan, at('2026-10-04T12:00:00')).repeat.at).toBe(at('2026-10-04T13:00:00'))
    const weekly = { ...plan, repeat: { kind: 'weekly', days: WEEKDAYS, minute: 540 } }
    expect(copyOf(weekly).repeat).toEqual(weekly.repeat)
  })
})
