import { t } from './i18n.js'

// カレンダーの帯の組み立て。API の 10 分ごとの件数から、帯（区間）・日ごとの切り分け・
// 重なったときの横並びを決める。画面の部品から切り離して単体で試せるようにしてある

export const BUCKET_MS = 10 * 60 * 1000
export const DAY_MS = 24 * 60 * 60 * 1000
// これより長く手が止まったら帯を分ける（1 セッションを開きっぱなしで翌日に再開することが多い）。
// サーバーの stats::GAP_SECS と同じ値にしておく（テストで確かめている）
export const GAP_MS = 30 * 60 * 1000

// その週の月曜 0:00（端末の時刻帯）
export function weekStart(date) {
  const d = new Date(date)
  d.setHours(0, 0, 0, 0)
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7))
  return d.getTime()
}

export function addDays(ms, n) {
  const d = new Date(ms)
  d.setDate(d.getDate() + n)
  return d.getTime()
}

// [[unix秒, 件数], …] → [{start, end, cells: [{t, n}]}]（ms）
export function segments(buckets) {
  const cells = buckets
    .map(([t, n]) => ({ t: t * 1000, n }))
    .sort((a, b) => a.t - b.t)
  const out = []
  for (const c of cells) {
    const last = out[out.length - 1]
    if (last && c.t - last.end <= GAP_MS) {
      last.cells.push(c)
      last.end = c.t + BUCKET_MS
      continue
    }
    out.push({ start: c.t, end: c.t + BUCKET_MS, cells: [c] })
  }
  return out
}

// 帯を日ごとに切る。from から days 日分（日表示は 1、週は 7、月は 42）。day は from から何日目か
export function splitByDay(seg, from, days = 7) {
  const out = []
  for (let day = 0; day < days; day++) {
    const ds = addDays(from, day)
    const de = addDays(from, day + 1)
    const start = Math.max(seg.start, ds)
    const end = Math.min(seg.end, de)
    if (start >= end) continue
    out.push({ day, start, end, dayStart: ds, cells: seg.cells.filter((c) => c.t >= start && c.t < end) })
  }
  return out
}

// 同じ日に重なる帯を横に並べる。重なりの塊ごとに列数を決め、各帯に lane / lanes を付ける
export function assignLanes(pieces) {
  const byDay = new Map()
  for (const p of pieces) {
    if (!byDay.has(p.day)) byDay.set(p.day, [])
    byDay.get(p.day).push(p)
  }
  for (const list of byDay.values()) {
    list.sort((a, b) => a.start - b.start || b.end - a.end)
    let cluster = []
    let clusterEnd = -Infinity
    const close = () => {
      const lanes = Math.max(0, ...cluster.map((p) => p.lane)) + 1
      for (const p of cluster) p.lanes = lanes
    }
    for (const p of list) {
      if (cluster.length && p.start >= clusterEnd) {
        close()
        cluster = []
      }
      const used = new Set(cluster.filter((q) => q.end > p.start).map((q) => q.lane))
      let lane = 0
      while (used.has(lane)) lane++
      p.lane = lane
      cluster.push(p)
      clusterEnd = Math.max(clusterEnd, p.end)
    }
    if (cluster.length) close()
  }
  return pieces
}

// 発言量 → 濃さ（0.15〜0.9）。件数の差が大きいので対数で寄せる
export function density(n, max) {
  if (max <= 0) return 0.15
  return 0.15 + 0.75 * Math.min(1, Math.log1p(n) / Math.log1p(max))
}

export function fmtTokens(n) {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`
  return String(n)
}

// API 単価換算の金額。1 ドル未満は 2 桁、それ以上は 1 桁
export function fmtUsd(usd) {
  if (usd == null) return '—'
  return usd < 1 ? `$${usd.toFixed(2)}` : `$${usd.toFixed(1)}`
}

// 秒 → 「3.5 時間」「45 分」
export function fmtHours(secs) {
  if (secs < 3600) return t('minutes', { n: Math.round(secs / 60) })
  return t('hours', { n: (secs / 3600).toFixed(1) })
}

// 帯の区間に入る金額の合計。usd は [[unix秒, USD], …]
export function sumUsd(usd, start, end) {
  return usd.filter(([t]) => t * 1000 >= start && t * 1000 < end).reduce((a, [, v]) => a + v, 0)
}

export function fmtDuration(ms) {
  const m = Math.round(ms / 60000)
  if (m < 60) return t('minutes', { n: m })
  return t('hoursMinutes', { h: Math.floor(m / 60), m: m % 60 })
}

// 小さな月のカレンダーに並べる日（月曜始まり・6 週 = 42 日）。その月の 1 日を含む週の月曜から
export function monthGrid(year, month) {
  const first = weekStart(new Date(year, month, 1).getTime())
  return Array.from({ length: 42 }, (_, i) => addDays(first, i))
}

// その日の 0:00（端末の時刻帯）
export function startOfDay(ms) {
  const d = new Date(ms)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

// 月表示の 1 マスに入れる、その日のセッション（セッションごとに 1 件、始まりの早い順）
export function dayEntries(sessions, gridFrom, days) {
  const out = Array.from({ length: days }, () => new Map())
  for (const s of sessions) {
    for (const seg of segments(s.buckets)) {
      for (const p of splitByDay(seg, gridFrom, days)) {
        const cur = out[p.day].get(s.id)
        if (!cur || p.start < cur.start) out[p.day].set(s.id, { session: s, start: p.start })
      }
    }
  }
  return out.map((m) => [...m.values()].sort((a, b) => a.start - b.start))
}

// 1 回ごとの予定は、この長さの枠で出す（長さを持たないので、題が読める高さにする）
export const PLAN_MS = 30 * 60 * 1000

// サーバーが返すこれからの予定 → 日ごとの枠。時間ごとの予定は end（その日の時間帯の終わり）まで帯にする
export function planPieces(plans, from, days) {
  const out = []
  for (const plan of plans) {
    for (let day = 0; day < days; day++) {
      const ds = addDays(from, day)
      const de = addDays(from, day + 1)
      if (plan.start < ds || plan.start >= de) continue
      out.push({ day, dayStart: ds, start: plan.start, end: Math.min(plan.end ?? plan.start + PLAN_MS, de), plan })
    }
  }
  return out
}
