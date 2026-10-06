// agent-calendar serve（Rust）が出す API。
// POST は JSON 本文を付ける（サーバーは JSON でない POST を受けない）
import { maskData, unmask } from './masking.js'

// スクショ用の表示（中身を作り物に置き換える）。切り替えたら読み込み直す
const MASK_KEY = 'agent-calendar.mask'
export const masked = (() => {
  try {
    return localStorage.getItem(MASK_KEY) === '1'
  } catch {
    return false
  }
})()
export function setMasked(on) {
  try {
    localStorage.setItem(MASK_KEY, on ? '1' : '0')
  } catch {
    // 覚えられなくても、この読み込みのあいだだけは切り替わる
  }
  location.reload()
}
// 作り物の値で保存・送信すると本物のデータが書き換わるので、スクショ用の表示のあいだは書き込みを止める
const MASK_ERROR = 'Turn off screenshot mode to change data (スクショ用の表示を切ってから操作してください)'

const j = async (path, init) => {
  if (masked && init?.method === 'POST') throw new Error(MASK_ERROR)
  const r = await fetch(path, init)
  if (!r.ok) throw new Error(await r.text() || `${path}: ${r.status}`)
  const data = await r.json()
  return masked ? maskData(data) : data
}
const post = (path, body = {}) =>
  j(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })

export const getWeek = (from, to) => j(`/api/week?from=${from}&to=${to}`)
// machines（ホストの id の配列。手元は空文字）を渡すとそのホストだけ。null なら全部
export const getStats = (from, to, machines = null) =>
  j(`/api/stats?from=${from}&to=${to}${machines == null ? '' : `&machines=${encodeURIComponent(JSON.stringify(machines))}`}`)
// 期間（日・週・月）の要約を作って保存する。machines は集計と同じホストの絞り込み（null なら全部）
export const summarizePeriod = (from, to, machines = null) => post('/api/period-summary', { from, to, machines })
export const getSession = (id) => j(`/api/session/${encodeURIComponent(id)}`)
export const rescan = () => post('/api/rescan')
export const setProject = (repo, project) => post('/api/repo-project', { repo, project })
export const resumeSession = (id) => post(`/api/session/${encodeURIComponent(id)}/resume`)
// 実行中の Claude からの許可の問い合わせに答える
// remember なら、このリポジトリでは今後も許可（ルールを .claude/settings.local.json に足す）
export const answerPermission = (requestId, allow, remember = false) =>
  post('/api/permission', { request_id: requestId, allow, remember })
// セッションの変更: 変わったファイル（commit を渡すとそのコミット）と 1 ファイルの差分
const q = (commit, path) =>
  [commit && `commit=${encodeURIComponent(commit)}`, path && `path=${encodeURIComponent(path)}`].filter(Boolean).join('&')
export const getChanges = (id, commit = null) => j(`/api/session/${encodeURIComponent(id)}/changes?${q(commit)}`)
// スクショ用の表示では、作り物のパスを本物に戻して問い合わせる
export const getDiff = (id, path, commit = null) => j(`/api/session/${encodeURIComponent(id)}/diff?${q(commit, unmask(path))}`)
// そのセッションで、画面の答えを待っている許可の問い合わせ（会話を開き直したときに出し直す）
export const getAsks = (id) => j(`/api/session/${encodeURIComponent(id)}/asks`)
// 実行中の指示を止める（画面の「中断」）。実行中でなければ失敗が返る
export const stopInstruction = (id) => post(`/api/session/${encodeURIComponent(id)}/stop`)
// ピン留め
export const getPins = () => j('/api/pins')
export const setPin = (id, pinned) => post(`/api/session/${encodeURIComponent(id)}/pin`, { pinned })
// 完了の印（要約が途中でも、残りを片付けたものに付ける。セッションがそのあと動いたら効かなくなる）
export const setFinished = (id, finished) => post(`/api/session/${encodeURIComponent(id)}/finished`, { finished })
// いま要約する（止まっていなくても、要約済みでも作り直す）
export const summarizeSession = (id) => post(`/api/session/${encodeURIComponent(id)}/summarize`)
export const getSettings = () => j('/api/settings')
export const saveSettings = (s) => post('/api/settings', s)
// 新しい版: 確かめた結果・今すぐ確かめる・入れ替える（systemd の下なら再起動する）
export const getUpdate = () => j('/api/update')
export const checkUpdate = () => post('/api/update/check')
export const installUpdate = () => post('/api/update/install')
// 会話の履歴。新しい側から limit 件。end を渡すとそれより前（古い側）
export const getTranscript = (id, end = null, limit = 200) =>
  j(`/api/session/${encodeURIComponent(id)}/transcript?limit=${limit}${end == null ? '' : `&end=${end}`}`)

// 途中経過を 1 行ずつ（NDJSON）返す POST。届いた順に onEvent へ渡す
async function streamPost(path, body, onEvent) {
  if (masked) throw new Error(MASK_ERROR)
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) throw new Error(await r.text() || `${path}: ${r.status}`)
  const reader = r.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buf += decoder.decode(value, { stream: true })
    let i
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).trim()
      buf = buf.slice(i + 1)
      if (line) onEvent(JSON.parse(line))
    }
  }
}

// 続きの指示を送る。途中経過（{ kind: 'text' | 'tool' | 'done', … }）を届いた順に onEvent へ渡す。
// images は [{ media_type, data(base64) }]
export const sendInstruction = (id, prompt, mode, onEvent, images = []) =>
  streamPost(`/api/session/${encodeURIComponent(id)}/send`, { prompt, mode, images }, onEvent)

// リポジトリ: 設定で選んだ組織の GitHub のリポジトリと手元の clone。refresh で GitHub から取り直す
export const getRepos = (refresh = false) => j(`/api/repos${refresh ? '?refresh=true' : ''}`)
// そのリポジトリのセッション（このマシンのもの、新しい順）
export const getRepoSessions = (repo) => j(`/api/repo-sessions?repo=${encodeURIComponent(unmask(repo))}`)
export const cloneRepo = (owner, name, root) => post('/api/repos/clone', { owner, name, root })
// リポジトリで新しいセッションを始める。最初に { kind: 'session', id } が届く
// リポジトリのメニュー: 片付けられる作業場所（git worktree）と手元のブランチ、その操作
export const getRepoManage = (dir) => j(`/api/repos/manage?dir=${encodeURIComponent(unmask(dir))}`)
export const removeWorktree = (dir, path) => post('/api/repos/worktree/remove', { dir, path })
export const deleteBranch = (dir, branch, force) => post('/api/repos/branch/delete', { dir, branch, force })
export const fetchRepo = (dir) => post('/api/repos/fetch', { dir })
// 「ここで始める」で選べるモデルの候補（エージェントごと。cli_default は選ばなかったときに CLI が使うモデル）
export const getModels = () => j('/api/models')
// 「ここで始める」で出発点に選べるブランチ（default は origin の既定のブランチ。分からなければ null）
// スクショ用の表示では別のブランチが同じ作り物の名前になることがあるので、重なりは 1 つにする
export const getBranches = async (dir) => {
  const r = await j(`/api/repos/branches?dir=${encodeURIComponent(unmask(dir))}`)
  return { ...r, branches: [...new Set(r.branches)] }
}
export const startSession = (input, onEvent) => streamPost('/api/repos/start', input, onEvent)

// 予定
export const getSchedules = () => j('/api/schedules')
export const getDirs = () => j('/api/dirs')
export const saveSchedule = (id, input) => post(id == null ? '/api/schedules' : `/api/schedules/${id}`, input)
export const deleteSchedule = (id) => post(`/api/schedules/${id}/delete`)
export const runSchedule = (id) => post(`/api/schedules/${id}/run`)
