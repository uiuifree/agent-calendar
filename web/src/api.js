// agent-calendar serve（Rust）が出す API。
// POST は JSON 本文を付ける（サーバーは JSON でない POST を受けない）
const j = async (path, init) => {
  const r = await fetch(path, init)
  if (!r.ok) throw new Error(await r.text() || `${path}: ${r.status}`)
  return r.json()
}
const post = (path, body = {}) =>
  j(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })

export const getWeek = (from, to) => j(`/api/week?from=${from}&to=${to}`)
// machines（ホストの id の配列。手元は空文字）を渡すとそのホストだけ。null なら全部
export const getStats = (from, to, machines = null) =>
  j(`/api/stats?from=${from}&to=${to}${machines == null ? '' : `&machines=${encodeURIComponent(JSON.stringify(machines))}`}`)
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
export const getDiff = (id, path, commit = null) => j(`/api/session/${encodeURIComponent(id)}/diff?${q(commit, path)}`)
// そのセッションで、画面の答えを待っている許可の問い合わせ（会話を開き直したときに出し直す）
export const getAsks = (id) => j(`/api/session/${encodeURIComponent(id)}/asks`)
// ピン留め
export const getPins = () => j('/api/pins')
export const setPin = (id, pinned) => post(`/api/session/${encodeURIComponent(id)}/pin`, { pinned })
// いま要約する（止まっていなくても、要約済みでも作り直す）
export const summarizeSession = (id) => post(`/api/session/${encodeURIComponent(id)}/summarize`)
export const getSettings = () => j('/api/settings')
export const saveSettings = (s) => post('/api/settings', s)
// 会話の履歴。新しい側から limit 件。end を渡すとそれより前（古い側）
export const getTranscript = (id, end = null, limit = 200) =>
  j(`/api/session/${encodeURIComponent(id)}/transcript?limit=${limit}${end == null ? '' : `&end=${end}`}`)

// 途中経過を 1 行ずつ（NDJSON）返す POST。届いた順に onEvent へ渡す
async function streamPost(path, body, onEvent) {
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
export const getRepoSessions = (repo) => j(`/api/repo-sessions?repo=${encodeURIComponent(repo)}`)
export const cloneRepo = (owner, name, root) => post('/api/repos/clone', { owner, name, root })
// リポジトリで新しいセッションを始める。最初に { kind: 'session', id } が届く
export const startSession = (input, onEvent) => streamPost('/api/repos/start', input, onEvent)

// 予定
export const getSchedules = () => j('/api/schedules')
export const getDirs = () => j('/api/dirs')
export const saveSchedule = (id, input) => post(id == null ? '/api/schedules' : `/api/schedules/${id}`, input)
export const deleteSchedule = (id) => post(`/api/schedules/${id}/delete`)
export const runSchedule = (id) => post(`/api/schedules/${id}/run`)
