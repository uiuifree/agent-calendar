// エージェントの返答をマークダウンとして描く。記録の中の HTML はタグとして働かせず文字のまま出す
// （html: false）。javascript: などのリンクは markdown-it が既定で無効にする
import MarkdownIt from 'markdown-it'
import { t } from './i18n.js'

const md = new MarkdownIt({ html: false, linkify: true, breaks: true })

// リンクは新しいタブで、開いた先からこの画面を触れないように
const defaultLink = md.renderer.rules.link_open ?? ((tokens, i, opts, env, self) => self.renderToken(tokens, i, opts))
md.renderer.rules.link_open = (tokens, i, opts, env, self) => {
  tokens[i].attrSet('target', '_blank')
  tokens[i].attrSet('rel', 'noopener noreferrer')
  return defaultLink(tokens, i, opts, env, self)
}

// 画像は読み込まない（img にすると、会話を開くだけで外の URL を読みに行く）。押せば新しいタブで開くリンクとして「画像: alt」と出す
md.renderer.rules.image = (tokens, i, opts, env, self) => {
  const token = tokens[i]
  const src = token.attrGet('src')
  const alt = self.renderInlineAsText(token.children, opts, env)
  const label = alt ? t('conv.image', { alt }) : t('conv.imageNoAlt')
  return `<a href="${md.utils.escapeHtml(src)}" target="_blank" rel="noopener noreferrer">${md.utils.escapeHtml(label)}</a>`
}

export const renderMarkdown = (text) => md.render(text ?? '')
