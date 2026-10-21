// 集計の CSV。CLI の `agent-calendar stats --csv`（stats::table）と同じ形:
// 1 行 = 1 リポジトリ。時間は時間単位（小数 1 桁）、金額は小数 2 桁で、単価の分からないモデルを含めば末尾に +
const field = (s) => (/[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s)

export function statsCsv(stats) {
  const rows = [['project', 'repository', 'hours', 'sessions', 'usd'].join(',')]
  for (const p of stats.projects ?? []) {
    for (const r of p.repos) {
      rows.push(
        [field(p.project), field(r.repo), (r.secs / 3600).toFixed(1), String(r.sessions), `${r.usd.toFixed(2)}${r.unpriced.length ? '+' : ''}`].join(','),
      )
    }
  }
  return `${rows.join('\n')}\n`
}

// 文字列をファイルとしてダウンロードさせる
export function download(name, text, type = 'text/csv') {
  const url = URL.createObjectURL(new Blob([text], { type }))
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  URL.revokeObjectURL(url)
}
