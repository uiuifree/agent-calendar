<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { getRepoSessions, getSettings, getStats, getTodo, setFinished, setProject, summarizePeriod, summarizeSession } from '../api.js'
import { statsCsv, download } from '../csv.js'
import { addDays, fmtHours, fmtTokens, fmtUsd, weekStart } from '../layout.js'
import { renderMarkdown } from '../markdown.js'
import { splitSummary } from '../period.js'
import { repoColor } from '../colors.js'
import { todoWithUndo } from '../todo.js'
import { locale, t } from '../i18n.js'

// 案件ごとの作業時間と API 単価換算の金額。案件はリポジトリごとにここで選ぶ（推測では埋めない）
// 同じ期間の残タスク（要約が途中か、残っていることが書かれているセッション）も下に並べる
const props = defineProps({
  day: { type: Number, required: true }, // 表示の基準の日（その日の 0:00）
  mode: { type: String, required: true }, // day / week / month（‹ › の送り幅も変わるので App が持つ）
  version: { type: Number, required: true }, // 読み直すたびに増える。増えたら取り直す
  hosts: { type: Array, default: null }, // 選んだホストの id。null = すべて
  hiddenRepos: { type: Array, default: () => [] }, // サイドバーでチェックを外したリポジトリ（集計から外す）
})
const emit = defineEmits(['update:mode', 'open-session', 'changed'])
const MODES = ['day', 'week', 'month']

const range = computed(() => {
  if (props.mode === 'day') return [props.day, addDays(props.day, 1)]
  if (props.mode === 'week') return [weekStart(props.day), addDays(weekStart(props.day), 7)]
  const d = new Date(props.day)
  return [new Date(d.getFullYear(), d.getMonth(), 1).getTime(), new Date(d.getFullYear(), d.getMonth() + 1, 1).getTime()]
})
const label = computed(() => {
  const [a, b] = range.value.map((x) => new Date(x))
  if (props.mode === 'day') return a.toLocaleDateString(locale(), { year: 'numeric', month: 'short', day: 'numeric', weekday: 'short' })
  if (props.mode === 'month') return a.toLocaleDateString(locale(), { year: 'numeric', month: 'long' })
  return new Intl.DateTimeFormat(locale(), { month: 'short', day: 'numeric' }).formatRange(a, new Date(b.getTime() - 1))
})

const data = ref(null)
const error = ref('')
// 期間を続けて送ったときなど、前の問い合わせの答えが後から届いても、最新の問い合わせの答えだけを使う
let loadSeq = 0
async function load() {
  const mine = ++loadSeq
  error.value = ''
  try {
    const r = await getStats(...range.value, props.hosts, props.hiddenRepos)
    if (mine === loadSeq) data.value = r
  } catch (e) {
    if (mine === loadSeq) error.value = String(e)
  }
}
watch([range, () => props.version, () => props.hosts, () => props.hiddenRepos], load, { immediate: true, deep: true })

// モデル別・種類別（本体・サブエージェント・advisor）の内訳。キャッシュの書き込みは 5 分と 1 時間の合計
const cacheWrite = (tk) => tk.cache_5m + tk.cache_1h
const KINDS = ['main', 'subagent', 'advisor']
const kinds = computed(() => {
  const by = data.value?.by_kind ?? {}
  const sum = KINDS.reduce((a, k) => a + (by[k]?.usd ?? 0), 0)
  return KINDS.filter((k) => by[k]).map((k) => ({ k, ...by[k], pct: sum ? (by[k].usd / sum) * 100 : null }))
})

// いま表示中の案件 → リポジトリの行を CSV で書き出す（CLI の stats --csv と同じ形）
const csvName = () => `agent-calendar-${new Date(range.value[0]).toLocaleDateString('sv')}-${props.mode}.csv`
const exportCsv = () => download(csvName(), statsCsv(data.value))

// リポジトリの行を押すと、そのリポジトリのセッションをその場に開く（一度に 1 つ）
// 一覧はこのマシンのセッションだけを、期間を問わず新しい順に（/api/repo-sessions は 100 件まで）
const openRepo = ref(null)
const repoSessions = ref(null) // null = 読み込み中
const sessionsError = ref('')
async function toggleRepo(repo) {
  if (openRepo.value === repo) {
    openRepo.value = null
    return
  }
  openRepo.value = repo
  repoSessions.value = null
  sessionsError.value = ''
  try {
    const list = (await getRepoSessions(repo)).sessions
    if (openRepo.value === repo) repoSessions.value = list
  } catch (e) {
    if (openRepo.value === repo) sessionsError.value = String(e.message ?? e)
  }
}

// 候補はこれまでに割り当てた案件
const suggestions = computed(() => data.value?.known_projects ?? [])

// 期間の要約。作るのに 1 分ほどかかるので、作っているあいだに期間を変えたら、終わってから今の期間を読み直すだけにする
const summarizing = ref(false)
const summaryError = ref('')
async function doSummarize() {
  summarizing.value = true
  summaryError.value = ''
  try {
    await summarizePeriod(...range.value, props.hosts)
    await load()
  } catch (e) {
    summaryError.value = String(e.message ?? e)
  } finally {
    summarizing.value = false
  }
}
watch(range, () => (summaryError.value = ''))
// 端的なまとめだけを見せ、詳しい部分はたたむ（下の残タスクまですぐ届くように）
const periodParts = computed(() => (data.value?.period_summary ? splitSummary(data.value.period_summary.text) : null))
const summarizedAt = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })

// 残タスク。途中のものを先に、あとは最後に動いた順。色は上の表のリポジトリと同じ
const slots = computed(() => Object.fromEntries((data.value?.projects ?? []).flatMap((p) => p.repos).map((r) => [r.repo, r.slot])))
// 完了にした行は数秒だけ「完了にしました・取り消す」として残す（読み直して一覧から消えても、同じ場所に出す）
const UNDO_MS = 6000
const undoable = ref({}) // id → 完了にした行
const undoTimers = new Map() // id → 消すまでのタイマー
function dropUndo(id) {
  clearTimeout(undoTimers.get(id))
  undoTimers.delete(id)
  const { [id]: _, ...rest } = undoable.value
  undoable.value = rest
}
// 期間を変えたら、前の期間で完了にした行は出さない
watch(range, () => {
  for (const id of undoTimers.keys()) clearTimeout(undoTimers.get(id))
  undoTimers.clear()
  undoable.value = {}
})
onUnmounted(() => {
  for (const id of undoTimers.keys()) clearTimeout(undoTimers.get(id))
})
// 残タスクを「すべての期間」で見る。オンのあいだは期間を問わない残タスクを取る（ホストの絞り込みは集計と同じ）
const allTodo = ref(false)
const todoAll = ref(null) // { todo, unsummarized_list }
let todoSeq = 0
async function loadAll() {
  const mine = ++todoSeq
  try {
    const r = await getTodo(props.hosts, props.hiddenRepos)
    if (mine === todoSeq) todoAll.value = r
  } catch (e) {
    if (mine === todoSeq) error.value = String(e.message ?? e)
  }
}
watch([allTodo, () => props.version, () => props.hosts, () => props.hiddenRepos], () => allTodo.value && loadAll(), { deep: true })
const todoSource = computed(() => (allTodo.value ? todoAll.value : data.value))
const pending = computed(() => todoSource.value?.unsummarized_list ?? [])
// まだ要約していないセッションの一覧（注記を押すと開く）。「今すぐ要約」は要約を止めて起動していれば出さない
const showPending = ref(false)
const canSummarize = ref(false)
onMounted(async () => {
  try {
    canSummarize.value = (await getSettings()).summarizing
  } catch {
    canSummarize.value = false
  }
})
const busy = ref(null) // 要約・完了にしている id
async function summarizeOne(id) {
  busy.value = id
  try {
    await summarizeSession(id)
    emit('changed')
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    busy.value = null
  }
}
async function finishPending(id) {
  busy.value = id
  try {
    await setFinished(id, true)
    emit('changed')
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    busy.value = null
  }
}

const todoView = computed(() => todoWithUndo(todoSource.value?.todo, undoable.value))
const todo = computed(() => todoView.value.list)
const todoCount = computed(() => todoView.value.count)
const repoName = (repo) => repo.split('/').filter(Boolean).pop() ?? repo
const when = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
async function finish(x) {
  try {
    await setFinished(x.id, true)
    clearTimeout(undoTimers.get(x.id))
    undoable.value = { ...undoable.value, [x.id]: x }
    undoTimers.set(x.id, setTimeout(() => dropUndo(x.id), UNDO_MS))
    emit('changed') // カレンダーの状態も変わるので、App に読み直してもらう（version が増えてここも取り直す）
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}
async function undoFinish(id) {
  try {
    await setFinished(id, false)
    dropUndo(id)
    emit('changed')
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}

async function assign(repo, ev) {
  try {
    await setProject(repo, ev.target.value)
    await load()
  } catch (e) {
    error.value = String(e)
  }
}
</script>

<template>
  <div class="stats">
    <div class="bar">
      <div class="segmented">
        <button v-for="m in MODES" :key="m" :class="{ on: mode === m }" @click="emit('update:mode', m)">{{ t(`stats.${m}`) }}</button>
      </div>
      <span class="label num">{{ label }}</span>
      <span v-if="error" class="chip warn">{{ error }}</span>
    </div>

    <template v-if="data">
      <div class="cards">
        <div class="card">
          <span class="muted">{{ t('stats.activeTime') }}</span>
          <b class="num">{{ fmtHours(data.total.secs) }}</b>
        </div>
        <div class="card">
          <span class="muted">{{ t('apiPrice') }}</span>
          <b class="num" :title="data.total.unpriced ? t('unpricedHint') : null">{{ fmtUsd(data.total.usd) }}{{ data.total.unpriced ? '+' : '' }}</b>
        </div>
        <span class="spacer" />
        <button class="btn small csv" :disabled="!data.projects.length" @click="exportCsv">{{ t('stats.csv') }}</button>
      </div>

      <table class="table">
        <thead>
          <tr>
            <th>{{ t('stats.projectRepo') }}</th>
            <th class="r">{{ t('stats.activeTime') }}</th>
            <th class="r">{{ t('stats.sessions') }}</th>
            <th class="r">{{ t('apiPrice') }}</th>
            <th>{{ t('stats.assign') }}</th>
          </tr>
        </thead>
        <tbody v-for="p in data.projects" :key="p.project">
          <tr class="project">
            <td>{{ p.project || t('stats.unassigned') }}</td>
            <td class="r num">{{ fmtHours(p.secs) }}</td>
            <td class="r num">{{ p.sessions }}</td>
            <td class="r num" :title="p.unpriced ? t('unpricedHint') : null">{{ fmtUsd(p.usd) }}{{ p.unpriced ? '+' : '' }}</td>
            <td />
          </tr>
          <template v-for="r in p.repos" :key="r.repo">
          <tr class="repo" :class="{ open: openRepo === r.repo }">
            <td :title="r.repo">
              <button class="fold" :aria-expanded="openRepo === r.repo" :aria-label="t('stats.repoSessions', { repo: r.name })" @click="toggleRepo(r.repo)">
                <span class="caret">{{ openRepo === r.repo ? '▾' : '▸' }}</span><i class="dot" :style="{ background: repoColor(r.slot) }" />{{ r.name }}
              </button>
            </td>
            <td class="r num">{{ fmtHours(r.secs) }}</td>
            <td class="r num">{{ r.sessions }}</td>
            <td class="r num" :title="r.unpriced.length ? r.unpriced.join(', ') : ''">{{ fmtUsd(r.usd) }}{{ r.unpriced.length ? '+' : '' }}</td>
            <td>
              <input
                class="assign"
                list="aw-projects"
                :value="p.project"
                :placeholder="t('stats.unassigned')"
                :aria-label="t('stats.assignLabel', { repo: r.name })"
                @change="assign(r.repo, $event)"
              />
            </td>
          </tr>
          <tr v-if="openRepo === r.repo" class="sessions-row">
            <td colspan="5">
              <p v-if="sessionsError" class="chip warn">{{ sessionsError }}</p>
              <p v-else-if="!repoSessions" class="muted small">{{ t('detail.loading') }}</p>
              <template v-else>
                <p class="muted small scope">{{ t('stats.sessionsScope') }}</p>
                <p v-if="!repoSessions.length" class="muted small scope">{{ t('stats.noSessions') }}</p>
                <button v-for="x in repoSessions" :key="x.id" class="session" @click="emit('open-session', x.id)">
                  <span class="muted num when">{{ when(x.last_ts) }}</span>
                  <span class="stitle">{{ x.title || t('untitled') }}</span>
                  <span v-if="x.source === 'codex'" class="chip src">{{ t('source.codex') }}</span>
                  <span v-if="x.status" class="chip" :class="x.status">{{ t(`status.${x.status}`) }}</span>
                </button>
              </template>
            </td>
          </tr>
          </template>
        </tbody>
      </table>
      <datalist id="aw-projects">
        <option v-for="s in suggestions" :key="s" :value="s" />
      </datalist>

      <template v-if="data.by_model?.length">
        <h3 class="section-title">{{ t('stats.byModel') }} <span class="muted small">{{ t('apiPrice') }}</span></h3>
        <table class="table breakdown">
          <thead>
            <tr>
              <th>{{ t('stats.model') }}</th>
              <th class="r">{{ t('detail.amount') }}</th>
              <th class="r">{{ t('stats.input') }}</th>
              <th class="r">{{ t('stats.output') }}</th>
              <th class="r">{{ t('stats.cacheRead') }}</th>
              <th class="r">{{ t('stats.cacheWrite') }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="m in data.by_model" :key="m.model">
              <td class="mono">{{ m.model }}</td>
              <td class="r num" :title="m.unpriced ? t('unpricedHint') : null">{{ fmtUsd(m.usd) }}{{ m.unpriced ? '+' : '' }}</td>
              <td class="r num">{{ fmtTokens(m.tokens.input) }}</td>
              <td class="r num">{{ fmtTokens(m.tokens.output) }}</td>
              <td class="r num">{{ fmtTokens(m.tokens.cache_read) }}</td>
              <td class="r num">{{ fmtTokens(cacheWrite(m.tokens)) }}</td>
            </tr>
          </tbody>
        </table>
      </template>
      <template v-if="kinds.length">
        <h3 class="section-title">{{ t('stats.byKind') }}</h3>
        <table class="table breakdown">
          <thead>
            <tr><th /><th class="r">{{ t('detail.amount') }}</th><th class="r">{{ t('stats.share') }}</th></tr>
          </thead>
          <tbody>
            <tr v-for="k in kinds" :key="k.k">
              <td>{{ t(`detail.kind.${k.k}`) }}</td>
              <td class="r num" :title="k.unpriced.length ? k.unpriced.join(', ') : null">{{ fmtUsd(k.usd) }}{{ k.unpriced.length ? '+' : '' }}</td>
              <td class="r num">{{ k.pct == null ? '—' : `${k.pct.toFixed(0)}%` }}</td>
            </tr>
          </tbody>
        </table>
      </template>

      <h3 class="section-title">
        {{ t('stats.periodSummary') }}
        <span class="spacer" />
        <button class="btn small" :disabled="summarizing" @click="doSummarize">
          {{ summarizing ? t('detail.summarizing') : data.period_summary ? t('detail.resummarize') : t('detail.summarizeNow') }}
        </button>
      </h3>
      <p v-if="summaryError" class="chip warn">{{ summaryError }}</p>
      <template v-if="data.period_summary">
        <div class="md" v-html="renderMarkdown(periodParts.brief)" />
        <details v-if="periodParts.detail" class="more">
          <summary>{{ t('stats.periodDetail') }}</summary>
          <div class="md" v-html="renderMarkdown(periodParts.detail)" />
        </details>
        <p class="muted small">
          {{ t('detail.summarizedBy', { model: data.period_summary.model }) }}<template v-if="data.period_summary.cost_usd != null"> ({{ fmtUsd(data.period_summary.cost_usd) }})</template>
          · {{ summarizedAt(data.period_summary.created_at) }}
        </p>
      </template>
      <p v-else class="muted">{{ t('stats.noPeriodSummary') }}</p>

      <h3 class="section-title">
        {{ t('stats.todo', { n: todoCount }) }}
        <span class="spacer" />
        <label class="toggle small"><input v-model="allTodo" type="checkbox" /> {{ t('stats.allPeriods') }}</label>
      </h3>
      <ul v-if="todo.length" class="todo">
        <li v-for="x in todo" :key="x.id">
          <div v-if="undoable[x.id]" class="line">
            <span class="muted done">{{ t('stats.markedDone', { title: x.title || t('untitled') }) }}</span>
            <button class="btn small" @click="undoFinish(x.id)">{{ t('stats.undoDone') }}</button>
          </div>
          <template v-else>
          <div class="line">
            <i class="dot" :style="{ background: repoColor(slots[x.repo]) }" />
            <span class="muted repo" :title="x.repo">{{ repoName(x.repo) }}</span>
            <button class="link title" @click="emit('open-session', x.id)">{{ x.title || t('untitled') }}</button>
            <span class="chip" :class="x.status">{{ t(`status.${x.status}`) }}</span>
            <span class="muted num when">{{ when(x.last_ts) }}</span>
            <button class="btn small" @click="finish(x)">{{ t('detail.markDone') }}</button>
          </div>
          <p v-if="x.next" class="next">{{ x.next }}</p>
          </template>
        </li>
      </ul>
      <p v-else class="muted">{{ t('stats.noTodo') }}</p>
      <template v-if="pending.length">
        <button class="pending muted small" :aria-expanded="showPending" @click="showPending = !showPending">
          {{ showPending ? '▾' : '▸' }} {{ t(allTodo ? 'stats.unsummarizedAll' : 'stats.unsummarized', { n: pending.length }) }}
        </button>
        <ul v-if="showPending" class="todo">
          <li v-for="x in pending" :key="x.id">
            <div class="line">
              <i class="dot" :style="{ background: repoColor(slots[x.repo]) }" />
              <span class="muted repo" :title="x.repo">{{ repoName(x.repo) }}</span>
              <button class="link title" @click="emit('open-session', x.id)">{{ x.title || t('untitled') }}</button>
              <span class="muted num when">{{ when(x.last_ts) }}</span>
              <!-- 別のマシンの記録も手元に写してあるので、要約も完了の印もここからできる -->
              <button v-if="canSummarize" class="btn small" :disabled="busy !== null" @click="summarizeOne(x.id)">
                {{ busy === x.id ? t('detail.summarizing') : t('detail.summarizeNow') }}
              </button>
              <button class="btn small" :disabled="busy !== null" @click="finishPending(x.id)">{{ t('detail.markDone') }}</button>
            </div>
          </li>
        </ul>
      </template>

      <p class="muted foot">
        {{ t('stats.noteTime', { gap: data.gap_secs / 60 }) }}<br />
        {{ t('stats.noteCost') }}
      </p>
    </template>
  </div>
</template>

<style scoped>
.stats{max-width:960px; padding-top:8px; container-type:inline-size}
.bar{display:flex; align-items:center; gap:12px; margin-bottom:16px}
.label{font-size:16px; color:var(--ink)}
.segmented{display:inline-flex; border:1px solid var(--outline); border-radius:18px; overflow:hidden}
.segmented button{border:0; background:none; height:32px; padding:0 16px; color:var(--ink-soft); font-weight:500}
.segmented button + button{border-left:1px solid var(--outline)}
.segmented button.on{background:var(--accent-soft); color:#041e49}
.cards{display:flex; align-items:flex-end; gap:16px; margin-bottom:20px}
.cards .spacer{flex:1}
.card{display:flex; flex-direction:column; gap:2px; min-width:200px; padding:14px 18px; border:1px solid var(--rule); border-radius:12px}
.card .muted{font-size:13px}
.card b{font-size:28px; font-weight:400; color:var(--ink)}
.project td{font-weight:500; padding-top:16px}
.repo td:first-child{padding-left:28px; color:var(--ink-soft)}
.dot{display:inline-block; width:10px; height:10px; border-radius:50%; margin-right:8px}
.assign{width:10em; height:30px; border:1px solid var(--rule); border-radius:6px; background:var(--ground); padding:0 8px}
.assign:focus{border-color:var(--accent); outline:none}
.fold{display:inline-flex; align-items:center; border:0; background:none; padding:0; font:inherit; color:inherit; cursor:pointer; text-align:left}
.fold:hover{color:var(--accent)}
.caret{width:14px; margin-left:-16px; font-size:11px; color:var(--muted)}
.repo.open td{background:var(--accent-faint)}
.sessions-row td{padding:4px 12px 8px 28px; background:var(--panel)}
.scope{margin:2px 8px 4px}
.session{display:flex; align-items:center; gap:10px; width:100%; border:0; background:none; padding:5px 8px; border-radius:8px; text-align:left; min-width:0}
.session:hover{background:var(--hover)}
.session .when{flex:none; font-size:12px}
.session .stitle{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px}
.breakdown .mono{word-break:break-all}
.toggle{display:inline-flex; align-items:center; gap:6px; font-weight:400; color:var(--ink-soft); cursor:pointer}
.pending{border:0; background:none; padding:0; margin:8px 0 0; text-align:left; cursor:pointer}
.pending:hover{color:var(--accent)}
.foot{font-size:12px; margin-top:20px; line-height:1.7}
.section-title{margin-top:28px}
.section-title .spacer{flex:1}
.md{font-size:14px; line-height:1.7}
.md :deep(h2){font-size:15px; font-weight:500; margin:16px 0 4px}
.md :deep(h3){font-size:14px; font-weight:500; margin:12px 0 2px}
.md :deep(ul){margin:4px 0; padding-left:20px}
.md :deep(p){margin:4px 0}
.more{margin:4px 0 8px}
.more > summary{cursor:pointer; color:var(--accent); font-size:13px; font-weight:500}
.todo{list-style:none; padding:0; margin:0}
.todo li{padding:10px 0; border-top:1px solid var(--rule-soft)}
.todo li:first-child{border-top:0}
.todo .line{display:flex; align-items:center; gap:8px; min-width:0}
.todo .repo{white-space:nowrap; font-size:13px}
.todo .title{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; text-align:left;
  border:0; background:none; padding:0; color:var(--ink); font:inherit; cursor:pointer}
.todo .title:hover{color:var(--accent); text-decoration:underline}
.todo .when{white-space:nowrap; font-size:12px}
.todo .done{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px}
.todo .next{margin:4px 0 0 18px; font-size:13px; color:var(--ink-soft); white-space:pre-wrap}
.btn.small{height:28px; padding:0 10px; font-size:12px; white-space:nowrap}
.small{font-size:12px}
/* 数字は折り返さない（「40.3 時間」が 2 行に割れないように） */
.r{white-space:nowrap}
.repo td:first-child{white-space:nowrap}
/* 右にパネルが出るなどして狭いときは、セッション数の列を省き、割り当ての欄を縮める（右にはみ出さない） */
@container (max-width: 680px){
  .table th:nth-child(3), .table td:nth-child(3){display:none}
  .assign{width:7.5em}
  .cards .card{min-width:0; flex:1}
}
</style>
