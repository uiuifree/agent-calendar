// リポジトリ → 色。Google カレンダーの予定の色から 8 色を選び、dataviz の検証で隣どうしが
// 見分けやすい並びにした（ライト。色覚の差が出る Basil と Tomato の間はリポジトリ名の文字で補う）。
// 色の割り当て（slot）はサーバーが全期間のセッション数で決めるので、週をめくっても同じ色のまま
export const PALETTE = [
  '#039be5', // Peacock
  '#d50000', // Tomato
  '#0b8043', // Basil
  '#8e24aa', // Grape
  '#f4511e', // Tangerine
  '#33b679', // Sage
  '#3f51b5', // Blueberry
  '#e67c73', // Flamingo
]
export const OTHER = '#616161' // Graphite（9 番目以降のリポジトリ）

export const repoColor = (slot) => (slot == null ? OTHER : PALETTE[slot] ?? OTHER)

// 塗った帯の上の文字色。白で 4.5:1 に届かない色（明るい青・黄緑・朱・桃）は黒にする
export function textOn(hex) {
  return contrast('#ffffff', hex) >= 4.5 ? '#ffffff' : '#1f1f1f'
}

function lum(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

export function contrast(a, b) {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}
