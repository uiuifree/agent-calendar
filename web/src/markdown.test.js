import { describe, expect, it } from 'vitest'
import { setLang } from './i18n.js'
import { renderMarkdown } from './markdown.js'

describe('renderMarkdown', () => {
  it('見出し・箇条書き・表・コードを描く', () => {
    const h = renderMarkdown('## 見出し\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n`code`')
    expect(h).toContain('<h2>見出し</h2>')
    expect(h).toContain('<li>a</li>')
    expect(h).toContain('<table>')
    expect(h).toContain('<pre><code class="language-rust">')
    expect(h).toContain('<code>code</code>')
  })
  it('HTML は文字のまま、危ないリンクは無効', () => {
    const h = renderMarkdown('<script>alert(1)</script> <img src=x onerror=alert(1)> [x](javascript:alert(1))')
    expect(h).not.toContain('<script>')
    expect(h).not.toContain('<img')
    expect(h).toContain('&lt;script&gt;')
    expect(h).not.toContain('href="javascript:')
  })
  it('リンクは新しいタブ', () => {
    expect(renderMarkdown('https://example.com')).toContain('target="_blank" rel="noopener noreferrer"')
    expect(renderMarkdown(null)).toBe('')
  })
  it('画像は読み込まず、新しいタブで開くリンクにする', () => {
    setLang('ja')
    const h = renderMarkdown('![図 <1>](https://example.com/a.png?x=1&y=2)')
    expect(h).not.toContain('<img')
    expect(h).toContain('<a href="https://example.com/a.png?x=1&amp;y=2" target="_blank" rel="noopener noreferrer">画像: 図 &lt;1&gt;</a>')
    expect(renderMarkdown('![](https://example.com/b.png)')).toContain('>画像</a>')
    setLang('en')
    expect(renderMarkdown('![chart](https://example.com/c.png)')).toContain('>Image: chart</a>')
  })
})
