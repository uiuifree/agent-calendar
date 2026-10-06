import { expect, it } from 'vitest'
import { isWidths, panelWidth, withoutWidth } from './resize.js'

it('ポインタの位置からパネルの幅を決める。パネルもカレンダーも 320px は残す', () => {
  // 画面 1600px・サイドバーなし: 境目を 1000px に置けばパネルは 600px
  expect(panelWidth(1000, 1600, 0)).toBe(600)
  expect(panelWidth(1000.4, 1600, 0)).toBe(600)
  // 右へ寄せすぎても 320px より狭くならない
  expect(panelWidth(1590, 1600, 0)).toBe(320)
  // 左へ寄せすぎても、カレンダーに 320px 残す（サイドバー 256px を開いていればその分も）
  expect(panelWidth(0, 1600, 0)).toBe(1280)
  expect(panelWidth(0, 1600, 256)).toBe(1024)
  // 画面が狭くて両方は守れないときは、パネルの 320px を優先する
  expect(panelWidth(100, 600, 256)).toBe(320)
})

it('覚えた幅の形を確かめ、1 つ外せる', () => {
  expect(isWidths({})).toBe(true)
  expect(isWidths({ conversation: 720, changes: 1100 })).toBe(true)
  expect(isWidths(null)).toBe(false)
  expect(isWidths([720])).toBe(false)
  expect(isWidths({ conversation: '720' })).toBe(false)
  expect(isWidths({ conversation: 100 })).toBe(false)
  expect(withoutWidth({ conversation: 720, changes: 1100 }, 'conversation')).toEqual({ changes: 1100 })
  expect(withoutWidth({ changes: 1100 }, 'summary')).toEqual({ changes: 1100 })
})
