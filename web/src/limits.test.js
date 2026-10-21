import { beforeEach, expect, it } from 'vitest'
import { setLang } from './i18n.js'
import { asOfLabel, clockAt, resetLabel, statuslineSetup, usageLevel, windowName } from './limits.js'

beforeEach(() => setLang('ja'))
const MIN = 60000

it('枠の名前: 5 時間・7 日・期間の分からない枠は決まった名前、ほかは label のまま', () => {
  expect(windowName({ kind: 'five_hour', label: 'x' })).toBe('5 時間')
  expect(windowName({ kind: 'seven_day', label: 'x' })).toBe('7 日')
  expect(windowName({ kind: 'window', label: 'x' })).toBe('利用枠')
  expect(windowName({ kind: 'model', label: 'gemini-3-pro' })).toBe('gemini-3-pro')
  expect(windowName({ kind: 'quota', label: 'daily' })).toBe('daily')
})

it('使用率の色分け', () => {
  expect(usageLevel(null)).toBe('none')
  expect(usageLevel(0)).toBe('low')
  expect(usageLevel(69.9)).toBe('low')
  expect(usageLevel(70)).toBe('mid')
  expect(usageLevel(90)).toBe('high')
  expect(usageLevel(100)).toBe('high')
})

it('戻るまで: 相対と絶対の時刻', () => {
  const now = new Date(2026, 9, 24, 18, 44).getTime()
  const at = now + (2 * 60 + 13) * MIN
  expect(resetLabel(at, now)).toBe(`あと 2 時間 13 分（${clockAt(at)}）`)
  expect(clockAt(at)).toContain('20:57')
  expect(resetLabel(now + 30 * 1000, now)).toBe(`あと 1 分（${clockAt(now + 30 * 1000)}）`)
  expect(resetLabel(now + 3 * 60 * MIN, now)).toMatch(/^あと 3 時間（/)
  expect(resetLabel(now + (3 * 24 + 5) * 60 * MIN, now)).toMatch(/^あと 3 日 5 時間（/)
  expect(resetLabel(now + 3 * 24 * 60 * MIN, now)).toMatch(/^あと 3 日（/)
  expect(resetLabel(now, now)).toBe('戻っています')
  setLang('en')
  expect(resetLabel(now + 45 * MIN, now)).toMatch(/^resets in 45 min \(/)
})

it('いつの時点の値か', () => {
  const now = 1_000_000_000_000
  expect(asOfLabel(now - 20 * 1000, now)).toBe('いまの時点')
  expect(asOfLabel(now - 13 * MIN, now)).toBe('13 分前の時点')
  expect(asOfLabel(now - 5 * 60 * MIN, now)).toBe('5 時間前の時点')
  expect(asOfLabel(now - 17 * 24 * 60 * MIN, now)).toBe('17 日前の時点')
})

it('statusline の案内は Claude Code と Gemini だけ', () => {
  expect(statuslineSetup('claude')).toEqual({
    file: '~/.claude/settings.json',
    snippet: '"statusLine": {"type": "command", "command": "agent-calendar statusline --agent claude"}',
  })
  expect(statuslineSetup('gemini').file).toBe('~/.gemini/antigravity-cli/settings.json')
  expect(statuslineSetup('gemini').snippet).toContain('--agent gemini')
  expect(statuslineSetup('codex')).toBe(null)
})
