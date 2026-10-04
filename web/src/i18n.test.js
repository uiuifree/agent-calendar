import { describe, expect, it } from 'vitest'
import { _messages, setLang, t } from './i18n.js'

// 英語と日本語でキーがそろっていること（片方だけ足すと画面にキーがそのまま出る）
function keys(o, prefix = '') {
  return Object.entries(o).flatMap(([k, v]) => (typeof v === 'object' ? keys(v, `${prefix}${k}.`) : [`${prefix}${k}`]))
}

describe('i18n', () => {
  it('英語と日本語のキーがそろっている', () => {
    expect(keys(_messages.ja).sort()).toEqual(keys(_messages.en).sort())
  })
  it('差し込みと切り替え', () => {
    setLang('en')
    expect(t('detail.prompts', { n: 3 })).toBe('3 prompts')
    setLang('ja')
    expect(t('detail.prompts', { n: 3 })).toBe('依頼 3 件')
    expect(t('no.such.key')).toBe('no.such.key')
  })
})
