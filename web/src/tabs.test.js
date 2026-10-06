import { expect, it } from 'vitest'
import { closeTab, isTabs, keptTabs, openTab, patchTab } from './tabs.js'

const tab = (id, kept = false, inner = 'summary') => ({ id, kept, tab: inner })

it('開く: 仮のタブは入れ替わり、固定したタブは残って右に足される', () => {
  // 最初の 1 つ
  expect(openTab([], null, 'a')).toEqual([tab('a')])
  // 仮のタブを見ているときは、その場で入れ替わる
  expect(openTab([tab('a')], 'a', 'b')).toEqual([tab('b')])
  // 固定したタブを見ているときは、右に足す
  expect(openTab([tab('a', true)], 'a', 'b')).toEqual([tab('a', true), tab('b')])
  // 固定したタブを見ていても、ほかに仮のタブがあればそれと入れ替える（仮のタブを増やさない）
  expect(openTab([tab('a', true), tab('b')], 'a', 'c')).toEqual([tab('a', true), tab('c')])
  // 仮のタブが 2 つあるとき（固定を外した直後）は、表示中のほうを入れ替える
  expect(openTab([tab('a'), tab('b')], 'b', 'c')).toEqual([tab('a'), tab('c')])
  // もう開いているものは並びを変えない
  const open = [tab('a', true), tab('b')]
  expect(openTab(open, 'a', 'b')).toBe(open)
  // パネルを閉じていた（表示中が無い）ときも、仮のタブがあれば入れ替える
  expect(openTab([tab('a', true), tab('b')], null, 'c')).toEqual([tab('a', true), tab('c')])
})

it('開く: 中のタブは表示中のタブに合わせる', () => {
  expect(openTab([tab('a', true, 'conversation')], 'a', 'b')[1]).toEqual(tab('b', false, 'conversation'))
  expect(openTab([tab('a', false, 'changes')], 'a', 'b')).toEqual([tab('b', false, 'changes')])
  expect(openTab([tab('a', true, 'conversation')], null, 'b')[1].tab).toBe('summary')
})

it('閉じる: 表示中を閉じたら右隣、無ければ左隣。最後の 1 つなら null', () => {
  const three = [tab('a', true), tab('b', true), tab('c')]
  expect(closeTab(three, 'b', 'b')).toEqual({ tabs: [tab('a', true), tab('c')], active: 'c' })
  expect(closeTab(three, 'c', 'c')).toEqual({ tabs: [tab('a', true), tab('b', true)], active: 'b' })
  expect(closeTab([tab('a')], 'a', 'a')).toEqual({ tabs: [], active: null })
  // 表示していないタブを閉じても、表示中はそのまま
  expect(closeTab(three, 'a', 'c').active).toBe('a')
  // 無いタブは何もしない
  expect(closeTab(three, 'a', 'x')).toEqual({ tabs: three, active: 'a' })
})

it('項目の書き換えと、固定したタブだけ残すこと', () => {
  const two = [tab('a'), tab('b')]
  expect(patchTab(two, 'a', { kept: true })).toEqual([tab('a', true), tab('b')])
  expect(patchTab(two, 'b', { tab: 'conversation' })).toEqual([tab('a'), tab('b', false, 'conversation')])
  expect(patchTab(two, 'x', { kept: true })).toEqual(two)
  expect(keptTabs([tab('a', true), tab('b')])).toEqual([tab('a', true)])
})

it('覚えた並びの形を確かめる', () => {
  expect(isTabs([])).toBe(true)
  expect(isTabs([tab('a', true, 'conversation')])).toBe(true)
  expect(isTabs(null)).toBe(false)
  expect(isTabs([null])).toBe(false)
  expect(isTabs([{ id: 1, kept: true, tab: 'summary' }])).toBe(false)
  expect(isTabs([{ id: 'a', kept: 'yes', tab: 'summary' }])).toBe(false)
  expect(isTabs([{ id: 'a', kept: true }])).toBe(false)
})
