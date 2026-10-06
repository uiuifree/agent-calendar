import { describe, expect, it } from 'vitest'
import { splitSummary } from './period.js'

describe('splitSummary', () => {
  it('最初の節と、それ以降に分ける', () => {
    const text = '## 概要\n- a\n- b\n\n## リポジトリごと\n### app\n- c\n\n## 残タスク\n- d'
    expect(splitSummary(text)).toEqual({
      brief: '## 概要\n- a\n- b',
      detail: '## リポジトリごと\n### app\n- c\n\n## 残タスク\n- d',
    })
  })
  it('### は節の区切りにしない', () => {
    expect(splitSummary('## 概要\n### x\n- a')).toEqual({ brief: '## 概要\n### x\n- a', detail: '' })
  })
  it('見出しで始まらなければ分けずに全部見せる', () => {
    expect(splitSummary('前置き\n## 概要\n- a\n## 詳細\n- b')).toEqual({ brief: '前置き\n## 概要\n- a\n## 詳細\n- b', detail: '' })
  })
})
