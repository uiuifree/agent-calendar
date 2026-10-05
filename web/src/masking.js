import { lang } from './i18n.js'

// スクショ用の表示: 画面が受け取るデータの中身（リポジトリ名・題・文章・パス・差分など）を、それらしい作り物に置き換える。
// 時刻・費用・件数・状態・ツールの名前は本物のまま（使っている様子は伝わるように）。
// 同じ元の値は、開いているあいだ同じ作り物になる
const REPOS = ['web-app', 'api-server', 'mobile-app', 'docs-site', 'infra', 'cli-tool', 'data-pipeline', 'design-system',
  'auth-service', 'billing', 'search', 'notifications', 'admin-panel', 'analytics', 'landing-page', 'sdk', 'payments', 'chat',
  'scheduler', 'image-service', 'crawler', 'blog', 'storefront', 'inventory', 'reports', 'gateway', 'worker', 'mailer',
  'dashboard', 'feature-flags', 'i18n', 'ui-kit', 'event-bus', 'cache', 'importer', 'exporter', 'helpdesk', 'forms',
  'onboarding', 'audit-log', 'recommendations', 'maps', 'video', 'uploader', 'webhooks', 'status-page', 'playground', 'templates']
const TITLES = {
  ja: ['ログイン画面のバグを直す', '検索 API の応答を速くする', '設定画面を作り直す', 'テストの不安定さを調べる', '通知のメール本文を整える',
    'CSV の取り込みを足す', '依存ライブラリを更新する', 'README を書き直す', 'ダッシュボードのグラフを足す', '権限まわりを見直す',
    '支払いのエラー処理を足す', 'CI を速くする', '画像のアップロードを直す', 'API のドキュメントを書く', 'ページ送りを足す',
    'ダークモードに対応する', 'メール送信を非同期にする', 'ログの出力を整える', 'タイムゾーンの扱いを直す', '検索結果の並び順を変える',
    'キャッシュを入れる', '一覧画面を軽くする', 'エラー画面を作る', '型の定義を見直す', 'リリースの手順を整える',
    '入力チェックを足す', 'スマホの表示を直す', 'Webhook を受ける', '古いコードを消す', 'バックアップを自動にする'],
  en: ['Fix the login page bug', 'Speed up the search API', 'Rebuild the settings page', 'Investigate flaky tests', 'Polish notification emails',
    'Add CSV import', 'Update dependencies', 'Rewrite the README', 'Add charts to the dashboard', 'Review access control',
    'Handle payment errors', 'Make CI faster', 'Fix image uploads', 'Write API docs', 'Add pagination',
    'Support dark mode', 'Send emails asynchronously', 'Clean up logging', 'Fix time zone handling', 'Change search ordering',
    'Add caching', 'Speed up the list page', 'Add an error page', 'Tighten the types', 'Tidy the release steps',
    'Add input validation', 'Fix the mobile layout', 'Receive webhooks', 'Remove dead code', 'Automate backups'],
}
const WORDS = {
  ja: ['画面', 'の', '一覧', 'を', '確認', 'し', '、', 'テスト', 'が', '通る', 'こと', 'を', '確かめ', 'ました', '。', '設定', 'に', '項目', 'を',
    '足し', '、', '保存', 'の', '処理', 'を', '直し', 'ます', '。', '原因', 'は', '読み込み', 'の', '順番', 'でした', '。', '次', 'は', '画面', 'の', '表示', 'です', '。'],
  en: ['checked', 'the', 'list', 'view', 'and', 'confirmed', 'the', 'tests', 'pass.', 'added', 'a', 'setting', 'and', 'fixed', 'the', 'save', 'handler.',
    'the', 'cause', 'was', 'the', 'load', 'order.', 'next', 'up', 'is', 'the', 'detail', 'page.', 'updated', 'the', 'query', 'and', 'the', 'docs.'],
}
const CODE = ['const value = compute(input);', 'if (!item) return null;', 'items.push(next);', 'return result;', 'let total = 0;',
  'await save(record);', 'fn handle(req: Request) -> Response {', '}', 'for item in items {', 'logger.info("done");']
const FILES = ['src/app', 'src/api/handler', 'src/components/List', 'src/lib/util', 'tests/app', 'src/models/user', 'src/routes/index', 'docs/guide']

// 文字列 → 0 以上の整数（同じ文字列は同じ値）
export function hash(s) {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}
const pick = (list, s) => list[hash(s) % list.length]
const isJa = (s) => /[぀-ヿ一-鿿]/.test(s)
// 作り物の文章は画面の言語で作る（英語の画面に日本語の題が並ばないように）
const textLang = () => (lang.value === 'ja' ? 'ja' : 'en')

const ORGS = ['acme', 'example-org', 'demo-labs', 'sample-inc', 'octo-team']

// 名前は、出てきた順に重ならないよう割り当てる（足りなければ project-N のように番号で）
function assigner(list, spare) {
  const names = new Map()
  return (original) => {
    if (!names.has(original)) {
      const used = new Set(names.values())
      const start = hash(original) % list.length
      let name = null
      for (let i = 0; i < list.length && !name; i++) {
        const c = list[(start + i) % list.length]
        if (!used.has(c)) name = c
      }
      names.set(original, name ?? `${spare}-${names.size + 1}`)
    }
    return names.get(original)
  }
}
export const repoName = assigner(REPOS, 'project')
export const orgName = assigner(ORGS, 'org')

// 作り物 → 本物。差分やセッションの一覧を取りに行くときは、本物のパスに戻して問い合わせる
const real = new Map()
const remember = (fake, original) => {
  real.set(fake, original)
  return fake
}
export const unmask = (v) => real.get(v) ?? v
const base = (p) => p.replace(/\/+$/, '').split('/').pop() || p
// パスは元の絶対パスごとに 1 つ（末尾の名前が同じ別のリポジトリでも、同じ作り物にしない）
const paths = new Map()
export function fakePath(p) {
  if (!paths.has(p)) {
    const want = `/home/me/projects/${repoName(base(p))}`
    const taken = new Set(paths.values())
    let fake = want
    for (let n = 2; taken.has(fake); n++) fake = `${want}-${n}`
    paths.set(p, remember(fake, p))
  }
  return paths.get(p)
}
// 案件名
const PROJECTS = ['Project Alpha', 'Project Beta', 'Project Gamma', 'Project Delta', 'Project Epsilon', 'Project Zeta']
export const projectName = assigner(PROJECTS, 'Project')
export const fakeTitle = (s) => pick(TITLES[textLang()], s)

// 文章: 行ごとに、頭の印（見出し・箇条書き・番号・引用・表）は残して、中身を長さの近い作り物にする
export function fakeText(s) {
  const lang = textLang()
  let inCode = false
  return s
    .split('\n')
    .map((line, i) => {
      if (line.trimStart().startsWith('```')) {
        inCode = !inCode
        return line.trimStart().slice(0, 3)
      }
      if (!line.trim()) return line
      if (inCode) return pick(CODE, `${s}:${i}`)
      const [, mark, body] = /^(\s*(?:#{1,6} |[-*+] |\d+\. |> |\|)?)(.*)$/.exec(line)
      return mark + filler(lang, body.length, `${s}:${i}`)
    })
    .join('\n')
}
// 文の頭から始めて（「に項目を…」のように助詞から始めない）、英語は文の区切りのあとを大文字にする
function filler(lang, length, seed) {
  const words = WORDS[lang]
  const ends = (w) => /[.。]$/.test(w)
  const starts = words.map((_, i) => i).filter((i) => i === 0 || ends(words[i - 1]))
  let i = starts[hash(seed) % starts.length]
  let out = ''
  let capital = true
  const target = Math.max(4, Math.min(length, 400))
  while (out.length < target) {
    let w = words[i++ % words.length]
    if (lang === 'en' && capital) w = w[0].toUpperCase() + w.slice(1)
    capital = ends(w)
    out += lang === 'en' && out ? ` ${w}` : w
  }
  return out
}

// 差分: 行の種類（+・-・@@・ファイルの情報）は残して、中身を作り物のコードにする
// ファイルの情報の行（--- や +++）は @@ の見出しより前だけ。見出しの中では --- で始まる行も本文なので置き換える
export function fakePatch(patch) {
  let inHunk = false
  return patch
    .split('\n')
    .map((line, i) => {
      if (line.startsWith('diff ')) inHunk = false
      if (!inHunk && /^(diff |index |--- |\+\+\+ |new file|deleted file)/.test(line)) return line.replace(/(a|b)\/\S+/g, '$1/src/app')
      const m = /^(@@ [^@]*@@)/.exec(line)
      if (m) {
        inHunk = true
        return m[1]
      }
      if (!line || line.startsWith('\\')) return line
      return line[0] + pick(CODE, `${patch.length}:${i}:${line}`)
    })
    .join('\n')
}
// ファイルのパスも、別の本物が同じ作り物にならないように割り当てる（差分を取りに行くとき本物に戻すため）
const files = new Map()
const fakeFile = (p) => {
  if (!files.has(p)) {
    const ext = /\.[A-Za-z0-9]+$/.exec(p)?.[0] ?? ''
    const taken = new Set(files.values())
    let n = hash(p) % 7
    while (taken.has(`${pick(FILES, p)}${n}${ext}`)) n++
    files.set(p, remember(`${pick(FILES, p)}${n}${ext}`, p))
  }
  return files.get(p)
}
const fakeGithub = (url) => {
  const m = /^https:\/\/github\.com\/[^/]+\/([^/?#]+)(.*)$/.exec(url)
  const org = /^https:\/\/github\.com\/([^/]+)/.exec(url)?.[1]
  return m ? `https://github.com/${orgName(org)}/${repoName(m[1])}${m[2].replace(/tree\/[^?]+|compare\/[^?]+/, (x) => x.split('/')[0] + '/feature/example')}` : url
}

// 項目の名前ごとの置き換え。value は文字列
const RULES = {
  repo: (v) => (v.startsWith('https://github.com/') ? fakeGithub(v) : fakePath(v)),
  cwd: fakePath, dir: fakePath, local: fakePath, file: fakePath, path: fakeFile,
  title: fakeTitle, subject: fakeTitle, name: (v) => (isJa(v) || v.includes(' ') ? fakeTitle(v) : repoName(v)),
  text: fakeText, summary: fakeText, prompt: fakeText, description: fakeText, next: fakeText, bullets: fakeText,
  // ブランチ名か、GitHub のブランチのページ（詳細の github.branch）
  branch: fakeBranch,
  // 「ここで始める」で選ぶ出発点のブランチ（一覧と既定のブランチ）
  branches: fakeBranch, default: fakeBranch, default_branch: fakeBranch,
  // リポジトリのメニュー: ブランチを使っているフォルダ
  checked_out: fakePath,
  label: (v) => (v === 'WSL' || v === 'this PC' ? v : 'server'),
  owner: orgName, owners: orgName, github_owners: orgName, roots: () => '/home/me/projects', repo_roots: () => '/home/me/projects', dirs: fakePath,
  project: projectName, known_projects: projectName,
  url: fakeGithub, compare: fakeGithub, // 再開のコマンド: 先頭の cd のパス（空白があると '…' で囲まれている）を作り物にする
  resume_command: (v) =>
    v.replace(/^cd ('(?:[^']|'\\'')*'|\S+)/, (_, p) => `cd ${fakePath(p.startsWith("'") ? p.slice(1, -1).replaceAll("'\\''", "'") : p)}`),
  rule: (v) => v.replace(/\(.*\)/, '(npm test:*)'), patch: fakePatch, error: fakeText,
}

function fakeBranch(v) {
  if (v.startsWith('https://')) return fakeGithub(v)
  return v === 'main' || v === 'master' ? v : `feature/${pick(['login-fix', 'search-speedup', 'settings', 'csv-import'], v)}`
}

// 受け取ったデータ全体を、項目の名前を見ながらたどって置き換える。
// キーがパスの表（週のデータのリポジトリの表）は、キーも置き換える
// ツールの呼び出し（会話の履歴・途中経過）の name はツールの名前（Bash など）なので残す
const TOOL_KINDS = new Set(['tool_use', 'tool'])
export function maskData(data, key = '') {
  if (typeof data === 'string') return data && RULES[key] ? RULES[key](data) : data
  if (Array.isArray(data)) return data.map((v) => maskData(v, key))
  if (data && typeof data === 'object') {
    return Object.fromEntries(
      Object.entries(data).map(([k, v]) => {
        if (k === 'name' && TOOL_KINDS.has(data.kind)) return [k, v]
        return [k.startsWith('/') ? fakePath(k) : k, maskData(v, k.startsWith('/') ? key : k)]
      }),
    )
  }
  return data
}
