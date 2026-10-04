import { describe, expect, it } from 'vitest'
import { OTHER, PALETTE, contrast, repoColor, textOn } from './colors.js'

describe('colors', () => {
  it('slot が無い・範囲外なら Graphite', () => {
    expect(repoColor(0)).toBe(PALETTE[0])
    expect(repoColor(null)).toBe(OTHER)
    expect(repoColor(99)).toBe(OTHER)
  })
  it('文字色は白で読めなければ黒', () => {
    expect(textOn('#d50000')).toBe('#ffffff') // Tomato
    expect(textOn('#039be5')).toBe('#1f1f1f') // Peacock
    expect(textOn('#33b679')).toBe('#1f1f1f') // Sage
    for (const c of [...PALETTE, OTHER]) expect(contrast(textOn(c), c)).toBeGreaterThanOrEqual(4.5)
  })
})
