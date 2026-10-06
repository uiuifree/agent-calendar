import { nextTick } from 'vue'
import { afterEach, beforeEach, expect, it } from 'vitest'
import { remembered, rememberedMode } from './remembered.js'

// Node には localStorage が無いので、Map で代わりを置く
let store
beforeEach(() => {
  store = new Map()
  globalThis.localStorage = {
    getItem: (k) => store.get(k) ?? null,
    setItem: (k, v) => store.set(k, v),
  }
})
afterEach(() => {
  delete globalThis.localStorage
})

it('覚えた値を読み、変えたら保存する。形が合わないものと読めないものは既定', async () => {
  store.set('k', JSON.stringify('week'))
  const r = remembered('k', 'day', (v) => ['day', 'week'].includes(v))
  expect(r.value).toBe('week')
  r.value = 'day'
  await nextTick()
  expect(store.get('k')).toBe(JSON.stringify('day'))
  store.set('bad', JSON.stringify('year'))
  expect(remembered('bad', 'day', (v) => ['day', 'week'].includes(v)).value).toBe('day')
  store.set('broken', '{')
  expect(remembered('broken', 'day').value).toBe('day')
  expect(remembered('none', true).value).toBe(true)
})

it('許可の範囲: 既定は自動判定。選び直したら次からそれが初期値', async () => {
  expect(rememberedMode().value).toBe('auto')
  const m = rememberedMode()
  m.value = 'edit'
  await nextTick()
  expect(rememberedMode().value).toBe('edit')
  store.set('agent-calendar.mode', JSON.stringify('dangerous'))
  expect(rememberedMode().value).toBe('auto')
})

it('localStorage が無い環境でも既定で動き、保存の失敗で止まらない', async () => {
  delete globalThis.localStorage
  const r = remembered('k', 'day')
  expect(r.value).toBe('day')
  r.value = 'week'
  await nextTick()
  expect(r.value).toBe('week')
})
