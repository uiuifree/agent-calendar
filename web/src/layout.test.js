import { describe, expect, it } from 'vitest'
import { setLang } from './i18n.js'
import { PLAN_MS, dayEntries, planPieces, startOfDay, monthGrid, assignLanes, density, fmtDuration, fmtHours, fmtTokens, fmtUsd, segments, splitByDay, sumUsd, weekStart, addDays } from './layout.js'

const at = (s) => new Date(s).getTime()
const sec = (s) => at(s) / 1000

describe('weekStart', () => {
  it('月曜 0:00 に寄せる', () => {
    expect(weekStart(at('2026-10-03T15:00:00'))).toBe(at('2026-09-28T00:00:00'))
    expect(weekStart(at('2026-10-04T23:00:00'))).toBe(at('2026-09-28T00:00:00'))
    expect(weekStart(at('2026-09-28T00:00:00'))).toBe(at('2026-09-28T00:00:00'))
  })
})

describe('segments', () => {
  it('30 分以内の切れ目はつなぎ、それより長いと分ける', () => {
    const s = segments([
      [sec('2026-10-01T10:00:00'), 3],
      [sec('2026-10-01T10:40:00'), 1], // 10:10 終わりから 30 分 → つなぐ
      [sec('2026-10-01T11:30:00'), 2], // 10:50 終わりから 40 分 → 分ける
    ])
    expect(s.map((x) => [x.start, x.end])).toEqual([
      [at('2026-10-01T10:00:00'), at('2026-10-01T10:50:00')],
      [at('2026-10-01T11:30:00'), at('2026-10-01T11:40:00')],
    ])
    expect(s[0].cells.map((c) => c.n)).toEqual([3, 1])
  })
})

describe('splitByDay', () => {
  it('日をまたぐ帯を日ごとに切る', () => {
    const from = at('2026-09-28T00:00:00')
    const seg = segments([[sec('2026-09-28T23:50:00'), 1], [sec('2026-09-29T00:00:00'), 2]])[0]
    const parts = splitByDay(seg, from)
    expect(parts.map((p) => [p.day, p.cells.length])).toEqual([[0, 1], [1, 1]])
    expect(parts[1].dayStart).toBe(addDays(from, 1))
  })
  it('週の外は捨てる', () => {
    const seg = segments([[sec('2026-10-05T10:00:00'), 1]])[0]
    expect(splitByDay(seg, at('2026-09-28T00:00:00'))).toEqual([])
  })
})

describe('assignLanes', () => {
  it('重なりの塊ごとに列数を決める', () => {
    const p = (day, start, end) => ({ day, start, end })
    const a = p(0, 0, 10), b = p(0, 5, 15), c = p(0, 12, 20), d = p(0, 30, 40), e = p(1, 0, 10)
    assignLanes([a, b, c, d, e])
    expect([a.lane, b.lane, c.lane]).toEqual([0, 1, 0])
    expect([a.lanes, b.lanes, c.lanes]).toEqual([2, 2, 2])
    expect([d.lane, d.lanes, e.lane, e.lanes]).toEqual([0, 1, 0, 1])
  })
})

describe('表示用の整形', () => {
  it('density は対数で 0.15〜0.9', () => {
    expect(density(0, 0)).toBe(0.15)
    expect(density(0, 10)).toBeCloseTo(0.15)
    expect(density(10, 10)).toBeCloseTo(0.9)
    expect(density(100, 10)).toBeCloseTo(0.9)
  })
  it('トークン数と時間', () => {
    setLang('ja')
    expect(fmtTokens(26_300_000)).toBe('26.3M')
    expect(fmtTokens(1500)).toBe('1.5k')
    expect(fmtTokens(12)).toBe('12')
    expect(fmtDuration(45 * 60000)).toBe('45 分')
    expect(fmtDuration(79 * 60000)).toBe('1 時間 19 分')
    expect(fmtUsd(null)).toBe('—')
    expect(fmtUsd(0.034)).toBe('$0.03')
    expect(fmtUsd(12.34)).toBe('$12.3')
    expect(fmtHours(45 * 60)).toBe('45 分')
    expect(fmtHours(3.5 * 3600)).toBe('3.5 時間')
    setLang('en')
    expect(fmtHours(45 * 60)).toBe('45 min')
    expect(fmtDuration(79 * 60000)).toBe('1 h 19 min')
  })
  it('区間内の金額だけ足す', () => {
    expect(sumUsd([[10, 1], [20, 2], [30, 4]], 20_000, 30_000)).toBe(2)
    expect(sumUsd([], 0, 1)).toBe(0)
  })
})

describe('monthGrid', () => {
  it('その月の 1 日を含む週の月曜から 42 日', () => {
    const g = monthGrid(2026, 9) // 2026 年 10 月（1 日は木曜）
    expect(g).toHaveLength(42)
    expect(g[0]).toBe(at('2026-09-28T00:00:00'))
    expect(g[3]).toBe(at('2026-10-01T00:00:00'))
    expect(g[41]).toBe(at('2026-11-08T00:00:00'))
  })
})

describe('日表示・月表示', () => {
  it('startOfDay は 0:00', () => {
    expect(startOfDay(at('2026-10-04T15:30:00'))).toBe(at('2026-10-04T00:00:00'))
  })
  it('splitByDay は日数を指定できる', () => {
    const seg = segments([[sec('2026-10-05T10:00:00'), 1]])[0]
    expect(splitByDay(seg, at('2026-10-05T00:00:00'), 1).map((p) => p.day)).toEqual([0])
    expect(splitByDay(seg, at('2026-10-04T00:00:00'), 1)).toEqual([])
  })
  it('dayEntries は日ごとにセッションを 1 件ずつ、早い順', () => {
    const from = at('2026-10-01T00:00:00')
    const a = { id: 'a', buckets: [[sec('2026-10-01T09:00:00'), 1], [sec('2026-10-01T15:00:00'), 1], [sec('2026-10-02T23:50:00'), 1], [sec('2026-10-03T00:00:00'), 1]] }
    const b = { id: 'b', buckets: [[sec('2026-10-01T08:00:00'), 1]] }
    const e = dayEntries([a, b], from, 3)
    expect(e[0].map((x) => x.session.id)).toEqual(['b', 'a'])
    expect(e[0][1].start).toBe(at('2026-10-01T09:00:00'))
    expect(e[1].map((x) => x.session.id)).toEqual(['a'])
    expect(e[2].map((x) => x.session.id)).toEqual(['a'])
  })
  it('planPieces は予定を日ごとの枠に。長さの無い回は PLAN_MS、時間ごとは end まで', () => {
    const from = at('2026-10-05T00:00:00')
    const once = { id: 1, start: at('2026-10-06T09:00:00'), end: null }
    const band = { id: 2, start: at('2026-10-05T12:30:00'), end: at('2026-10-05T18:00:00') }
    const allDay = { id: 3, start: at('2026-10-05T00:00:00'), end: at('2026-10-06T00:00:00') }
    const outside = { id: 4, start: at('2026-10-08T09:00:00'), end: null }
    const p = planPieces([once, band, allDay, outside], from, 2)
    expect(p.map((x) => [x.plan.id, x.day])).toEqual([[1, 1], [2, 0], [3, 0]])
    expect(p[0].end - p[0].start).toBe(PLAN_MS)
    expect(p[0].dayStart).toBe(at('2026-10-06T00:00:00'))
    expect(p[1].end).toBe(at('2026-10-05T18:00:00'))
    expect(p[2].end).toBe(at('2026-10-06T00:00:00'))
  })
})
