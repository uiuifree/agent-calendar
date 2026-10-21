import { expect, it } from 'vitest'
import { canDrop, restoredText, sendOutcome } from './drafts.js'

it('空の預かりは捨てる。書きかけ・画像があるか、送っている最中なら残す', () => {
  expect(canDrop({ sending: 0, text: '', attachments: [] })).toBe(true)
  expect(canDrop({ sending: 0, text: 'x', attachments: [] })).toBe(false)
  expect(canDrop({ sending: 0, text: '', attachments: [{}] })).toBe(false)
  expect(canDrop({ sending: 1, text: '', attachments: [] })).toBe(false)
})

it('途中経過が届かずに失敗したら断られたものとして戻す。中断・途中まで進んだもの・成功は戻さない', () => {
  expect(sendOutcome({ kind: 'done', ok: false }, false)).toEqual({ failed: true, rejected: true })
  expect(sendOutcome({ kind: 'done', ok: false, stopped: true }, false)).toEqual({ failed: true, rejected: false })
  expect(sendOutcome({ kind: 'done', ok: false }, true)).toEqual({ failed: true, rejected: false })
  expect(sendOutcome({ kind: 'done', ok: true }, false)).toEqual({ failed: false, rejected: false })
  expect(sendOutcome({ kind: 'text', text: 'x' }, true)).toEqual({ failed: false, rejected: false })
  expect(sendOutcome(undefined, false)).toEqual({ failed: false, rejected: false })
})

it('戻した指示の後ろに、実行中に書き足した分を残す', () => {
  expect(restoredText('前の指示', '')).toBe('前の指示')
  expect(restoredText('前の指示', '  ')).toBe('前の指示')
  expect(restoredText('前の指示', '次')).toBe('前の指示\n\n次')
})
