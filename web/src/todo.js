// 集計の画面の残タスク。完了にした行は数秒だけ「完了にしました・取り消す」として同じ場所に残す
// （読み直して一覧から消えても出す）。途中のものを先に、あとは最後に動いた順

// todo は届いた残タスク、undoable は id → 完了にした行。
// 答えは並べた一覧と、見出しに出す件数（取り消せるあいだの行は数えない）
export function todoWithUndo(todo, undoable) {
  const list = [...(todo ?? []).filter((x) => !undoable[x.id]), ...Object.values(undoable)].sort(
    (a, b) => (a.status === 'wip' ? 0 : 1) - (b.status === 'wip' ? 0 : 1) || b.last_ts - a.last_ts,
  )
  return { list, count: list.length - Object.keys(undoable).length }
}
