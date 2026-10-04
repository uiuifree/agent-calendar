// URL と表示の状態の対応。カレンダー（日・週・月）は URL を変えず "/"、ほかの画面とセッションだけを URL に持つ。
// 戻る・進む・ブックマーク・共有で、集計・予定・リポジトリの画面と、開いているセッションに戻れるように
export const CALENDAR_VIEWS = ['day', 'week', 'month']

const TABS = ['summary', 'conversation', 'changes']

// URL → { view, owner, session, tab, full }。view が 'calendar' ならカレンダー（日・週・月のどれかは前回のまま）。
// tab（詳細パネルのタブ）と full（画面いっぱい）はセッションを開いているときだけ
export function parseRoute(pathname, search = '') {
  const [v, arg] = pathname.split('/').filter(Boolean).map(decodeURIComponent)
  const params = new URLSearchParams(search)
  const session = params.get('session') || null
  const view = { stats: 'stats', schedules: 'plans', repos: 'repos' }[v] ?? 'calendar'
  const tab = TABS.includes(params.get('tab')) ? params.get('tab') : 'summary'
  return {
    view,
    owner: view === 'repos' ? (arg ?? null) : null,
    session,
    tab: session ? tab : 'summary',
    full: Boolean(session) && params.get('full') === '1',
  }
}

// 表示の状態 → URL
export function formatRoute({ view, owner, session, tab = 'summary', full = false }) {
  const path = CALENDAR_VIEWS.includes(view)
    ? '/'
    : { stats: '/stats', plans: '/schedules', repos: owner ? `/repos/${encodeURIComponent(owner)}` : '/repos' }[view]
  if (!session) return path
  const q = [`session=${encodeURIComponent(session)}`]
  if (tab !== 'summary') q.push(`tab=${tab}`)
  if (full) q.push('full=1')
  return `${path}?${q.join('&')}`
}
