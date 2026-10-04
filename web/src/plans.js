// 予定の「いつ動くか」を文にする。days は月曜を 1 ビット目とする曜日の印
import { locale, t } from './i18n.js'

export const ALL_DAYS = 0b1111111
export const WEEKDAYS = 0b0011111
export const WEEKEND = 0b1100000

export function dayLabel(days) {
  if ((days & ALL_DAYS) === ALL_DAYS) return t('plans.everyDay')
  if (days === WEEKDAYS) return t('plans.weekdays')
  if (days === WEEKEND) return t('plans.weekend')
  const names = Array.from({ length: 7 }, (_, i) => new Date(2026, 9, 5 + i).toLocaleDateString(locale(), { weekday: 'short' }))
  return names.filter((_, i) => days & (1 << i)).join(t('plans.sep'))
}

const hm = (minute) => `${Math.floor(minute / 60)}:${String(minute % 60).padStart(2, '0')}`

export const everyLabel = (min) =>
  min % 60 === 0 ? t('plans.everyHours', { n: min / 60 }) : t('plans.everyMinutes', { n: min })

export function describe(repeat) {
  if (!repeat) return ''
  if (repeat.kind === 'once') {
    const d = new Date(repeat.at).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
    return t('plans.onceAt', { when: d })
  }
  if (repeat.kind === 'weekly') return `${dayLabel(repeat.days)} ${hm(repeat.minute)}`
  return `${dayLabel(repeat.days)} ${repeat.from_hour}:00–${repeat.to_hour}:00 ${everyLabel(repeat.every_min)}`
}

// 予定のコピー（id を持たないので、保存すると新しい予定になる）。止めていても動く状態で始め、
// 1 回だけの予定の時刻が過ぎていれば翌日の同じ時刻にする（過去の時刻では保存できないので）
export function copyOf(plan, now = Date.now()) {
  let repeat = plan.repeat
  if (repeat?.kind === 'once' && repeat.at <= now) {
    const d = new Date(now)
    const was = new Date(repeat.at)
    d.setDate(d.getDate() + 1)
    d.setHours(was.getHours(), was.getMinutes(), 0, 0)
    repeat = { ...repeat, at: d.getTime() }
  }
  const { id, runs, next_at, last_session_id, ...rest } = plan
  return { ...rest, name: t('plans.copyName', { name: plan.name }), enabled: true, repeat }
}
