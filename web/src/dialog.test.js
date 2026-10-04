import { expect, it } from 'vitest'
import { closesOnEscape } from './dialog.js'

it('Esc で閉じる。変換中の Esc とほかのキーでは閉じない', () => {
  expect(closesOnEscape({ key: 'Escape', isComposing: false })).toBe(true)
  expect(closesOnEscape({ key: 'Escape', isComposing: true })).toBe(false)
  expect(closesOnEscape({ key: 'Enter', isComposing: false })).toBe(false)
})
