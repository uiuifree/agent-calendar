// 期間の要約（マークダウン）を、最初に見せる端的なまとめ（最初の ## の節）と、たたんでおく詳しい部分（2 つめの ## から）に分ける
export function splitSummary(text) {
  const m = /^## /m.exec(text.slice(text.indexOf('## ') + 1))
  if (!text.startsWith('## ') || !m) return { brief: text, detail: '' }
  const at = text.indexOf('## ') + 1 + m.index
  return { brief: text.slice(0, at).trim(), detail: text.slice(at).trim() }
}
