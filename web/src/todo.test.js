import { expect, it } from 'vitest'
import { todoWithUndo } from './todo.js'

it('途中のものを先に、あとは新しい順', () => {
  const todo = [
    { id: 'a', status: 'pending', last_ts: 3 },
    { id: 'b', status: 'wip', last_ts: 1 },
    { id: 'c', status: 'pending', last_ts: 5 },
  ]
  const r = todoWithUndo(todo, {})
  expect(r.list.map((x) => x.id)).toEqual(['b', 'c', 'a'])
  expect(r.count).toBe(3)
  expect(todoWithUndo(null, {})).toEqual({ list: [], count: 0 })
})

it('完了にした行は読み直して消えても同じ場所に残し、件数には数えない', () => {
  const done = { id: 'c', status: 'pending', last_ts: 5 }
  // 読み直す前（まだ一覧にある）: 二重に出さない
  const before = todoWithUndo([done, { id: 'a', status: 'pending', last_ts: 3 }], { c: done })
  expect(before.list.map((x) => x.id)).toEqual(['c', 'a'])
  expect(before.count).toBe(1)
  // 読み直した後（一覧から消えた）: 同じ場所に出す
  const after = todoWithUndo([{ id: 'a', status: 'pending', last_ts: 3 }], { c: done })
  expect(after.list.map((x) => x.id)).toEqual(['c', 'a'])
  expect(after.count).toBe(1)
})
