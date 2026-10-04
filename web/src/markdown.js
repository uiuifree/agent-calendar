// エージェントの返答をマークダウンとして描く。記録の中の HTML はタグとして働かせず文字のまま出す
// （html: false）。javascript: などのリンクは markdown-it が既定で無効にする
import MarkdownIt from 'markdown-it'

const md = new MarkdownIt({ html: false, linkify: true, breaks: true })

// リンクは新しいタブで、開いた先からこの画面を触れないように
const defaultLink = md.renderer.rules.link_open ?? ((tokens, i, opts, env, self) => self.renderToken(tokens, i, opts))
md.renderer.rules.link_open = (tokens, i, opts, env, self) => {
  tokens[i].attrSet('target', '_blank')
  tokens[i].attrSet('rel', 'noopener noreferrer')
  return defaultLink(tokens, i, opts, env, self)
}

export const renderMarkdown = (text) => md.render(text ?? '')
