// 右の詳細パネルのタブ。開いているセッションの並び [{ id, kept, tab }] を扱う。
// kept は固定したタブ（ほかのセッションを開いても残る）。固定していないタブは「仮のタブ」で、
// 次にセッションを開くと中身が入れ替わる。tab はそのセッションで開いている中のタブ（概要・会話・変更）

// ブラウザに覚えた並びの形を確かめる（古い書式なら捨てる）
export const isTabs = (v) =>
  Array.isArray(v) && v.every((x) => x && typeof x.id === 'string' && typeof x.kept === 'boolean' && typeof x.tab === 'string')

// セッションを開く。もう開いていれば並びはそのまま。無ければ仮のタブと入れ替える
// （表示中のタブが仮ならそれ、違えば最初の仮のタブ）。仮のタブが無ければ右に足す。
// 中のタブは表示中のタブに合わせる（会話を見ながら別のセッションを開いたら、それも会話で開く）
export function openTab(tabs, activeId, id) {
  if (tabs.some((x) => x.id === id)) return tabs
  const active = tabs.find((x) => x.id === activeId)
  const fresh = { id, kept: false, tab: active?.tab ?? 'summary' }
  const spare = active && !active.kept ? active : tabs.find((x) => !x.kept)
  return spare ? tabs.map((x) => (x === spare ? fresh : x)) : [...tabs, fresh]
}

// タブを閉じる。表示中のタブを閉じたら右隣（無ければ左隣）を表示する。最後の 1 つなら null（パネルを閉じる）
export function closeTab(tabs, activeId, id) {
  const i = tabs.findIndex((x) => x.id === id)
  if (i < 0) return { tabs, active: activeId }
  const rest = tabs.filter((x) => x.id !== id)
  return { tabs: rest, active: id === activeId ? ((rest[i] ?? rest[i - 1])?.id ?? null) : activeId }
}

// 1 つのタブの項目を書き換える（固定の付け外し、中のタブの切り替え）
export const patchTab = (tabs, id, patch) => tabs.map((x) => (x.id === id ? { ...x, ...patch } : x))

// パネルを閉じたとき（表示中のタブが無くなったとき）は、仮のタブを捨てて固定したタブだけ残す
export const keptTabs = (tabs) => tabs.filter((x) => x.kept)
