import { describe, expect, it } from 'vitest'
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
})
