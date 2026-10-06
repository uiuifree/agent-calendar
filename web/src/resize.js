// 右の詳細パネルの幅をドラッグで変える（カレンダーとの境目を左右に動かす）。
// パネルは 320px より狭くせず、カレンダー側にも 320px は残す（サイドバーを開いているときはその分も引く）
export const MIN_PANEL = 320
export const MIN_CALENDAR = 320

// ポインタの位置（画面の左からの px）→ パネルの幅。画面が狭くて両方は守れないときは、パネルの 320px を優先する
export function panelWidth(pointerX, viewport, sidebar) {
  const max = Math.max(MIN_PANEL, viewport - sidebar - MIN_CALENDAR)
  return Math.round(Math.min(max, Math.max(MIN_PANEL, viewport - pointerX)))
}

// ブラウザに覚えた幅（中のタブごと: { conversation: 720, … }）の形を確かめる
export const isWidths = (v) =>
  v != null && typeof v === 'object' && !Array.isArray(v) && Object.values(v).every((n) => Number.isFinite(n) && n >= MIN_PANEL)

// 覚えた幅から 1 つ外す（その中のタブを既定の幅に戻す）
export const withoutWidth = (widths, tab) => Object.fromEntries(Object.entries(widths).filter(([k]) => k !== tab))
