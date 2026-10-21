// いま動いているもの（/api/running）と裏の処理（/api/status）を、画面の印に使う形にする

// セッション → 印（{ asks }）。実行中の印と許可待ちの数を、タブやピン留めの行に付けるため
export function runningMarks(running) {
  const marks = {}
  for (const r of running?.runs ?? []) {
    if (!r.session_id) continue
    marks[r.session_id] = { asks: (marks[r.session_id]?.asks ?? 0) + r.asks }
  }
  return marks
}

// 許可待ちの合計
export const askCount = (running) => (running?.runs ?? []).reduce((a, r) => a + r.asks, 0)

// ページのタブの題。許可待ちがあれば頭に件数を付ける
export const pageTitle = (base, asks) => (asks ? `(${asks}) ${base}` : base)

// 裏の処理の名前 → 種類とホスト。scan（手元の読み直し）・scan:<ホスト>・summarize・remote:<ホスト>
export function jobParts(name) {
  const [kind, ...rest] = name.split(':')
  return { kind, machine: rest.join(':') }
}

// 別マシンの取り込みに失敗しているホスト → 理由
export function hostErrors(jobs) {
  const out = {}
  for (const j of jobs ?? []) {
    const { kind, machine } = jobParts(j.name)
    if (kind === 'remote' && j.error) out[machine] = j.error
  }
  return out
}
