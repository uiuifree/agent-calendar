// 利用枠の画面の書き方: 枠の名前、使用率の色分け、戻るまでの時間と、値がいつの時点のものか
import { locale, t } from './i18n.js'

// 5 時間・7 日・期間の分からない枠は決まった名前、モデルごと・枠ごとのものは label のまま
export function windowName(w) {
  const fixed = { five_hour: 'limits.fiveHour', seven_day: 'limits.sevenDay', window: 'limits.window' }[w.kind]
  return fixed ? t(fixed) : w.label
}

// 使用率の色分け。null（分からない）は none、90% から high、70% から mid
export function usageLevel(pct) {
  if (pct == null) return 'none'
  if (pct >= 90) return 'high'
  if (pct >= 70) return 'mid'
  return 'low'
}

// 何分・何時間・何日。2 日に満たなければ時間と分、それより先は日と時間
function span(ms) {
  const m = Math.max(1, Math.ceil(ms / 60000))
  if (m < 60) return t('minutes', { n: m })
  if (m < 48 * 60) return m % 60 ? t('hoursMinutes', { h: Math.floor(m / 60), m: m % 60 }) : t('hours', { n: m / 60 })
  const h = Math.floor(m / 60)
  return h % 24 ? t('limits.daysHours', { d: Math.floor(h / 24), h: h % 24 }) : t('limits.days', { n: h / 24 })
}

// 時刻を「10/24 20:57」のように
export const clockAt = (ms) =>
  new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false })

// 戻るまで: 「あと 2 時間 13 分（10/24 20:57）」。もう過ぎていれば「戻っています」
export function resetLabel(resetsAt, now) {
  if (resetsAt <= now) return t('limits.resetPassed')
  return t('limits.resetIn', { span: span(resetsAt - now), at: clockAt(resetsAt) })
}

// 値がいつの時点か: 「13 分前の時点」。1 分に満たなければ「いまの時点」
export function asOfLabel(observedAt, now) {
  const ms = now - observedAt
  if (ms < 60000) return t('limits.asOfNow')
  const m = Math.floor(ms / 60000)
  if (m < 60) return t('limits.asOf', { ago: t('minutes', { n: m }) })
  const h = Math.floor(m / 60)
  if (h < 48) return t('limits.asOf', { ago: t('hours', { n: h }) })
  return t('limits.asOf', { ago: t('limits.days', { n: Math.floor(h / 24) }) })
}

// 値が無いとき、statusline に登録して取る案内（設定のファイルと足す 1 行）。案内の無いエージェントは null
const SETTINGS_FILE = { claude: '~/.claude/settings.json', gemini: '~/.gemini/antigravity-cli/settings.json' }
export function statuslineSetup(agent) {
  if (!SETTINGS_FILE[agent]) return null
  return {
    file: SETTINGS_FILE[agent],
    snippet: `"statusLine": {"type": "command", "command": "agent-calendar statusline --agent ${agent}"}`,
  }
}
