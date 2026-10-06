<script setup>
import { computed, ref, watch } from 'vue'
import { getStats, setFinished, setProject, summarizePeriod } from '../api.js'
import { addDays, fmtHours, fmtUsd, weekStart } from '../layout.js'
import { renderMarkdown } from '../markdown.js'
import { splitSummary } from '../period.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'

// 案件ごとの作業時間と API 単価換算の金額。案件はリポジトリごとにここで選ぶ（推測では埋めない）
// 同じ期間の残タスク（要約が途中か、残っていることが書かれているセッション）も下に並べる
const props = defineProps({
  day: { type: Number, required: true }, // 表示の基準の日（その日の 0:00）
  mode: { type: String, required: true }, // day / week / month（‹ › の送り幅も変わるので App が持つ）
  version: { type: Number, required: true }, // 読み直すたびに増える。増えたら取り直す
  hosts: { type: Array, default: null }, // 選んだホストの id。null = すべて
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
async function load() {
  error.value = ''
  try {
    data.value = await getStats(...range.value, props.hosts)
  } catch (e) {
    error.value = String(e)
  }
}
watch([range, () => props.version, () => props.hosts], load, { immediate: true, deep: true })

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
const todo = computed(() =>
  [...(data.value?.todo ?? [])].sort((a, b) => (a.status === 'wip' ? 0 : 1) - (b.status === 'wip' ? 0 : 1) || b.last_ts - a.last_ts),
)
const repoName = (repo) => repo.split('/').filter(Boolean).pop() ?? repo
const when = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
async function finish(id) {
  try {
    await setFinished(id, true)
    emit('changed') // カレンダーの状態も変わるので、App に読み直してもらう（version が増えてここも取り直す）
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
          <b class="num">{{ fmtUsd(data.total.usd) }}</b>
        </div>
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
            <td class="r num">{{ fmtUsd(p.usd) }}</td>
            <td />
          </tr>
          <tr v-for="r in p.repos" :key="r.repo" class="repo">
            <td :title="r.repo"><i class="dot" :style="{ background: repoColor(r.slot) }" />{{ r.name }}</td>
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
        </tbody>
      </table>
      <datalist id="aw-projects">
        <option v-for="s in suggestions" :key="s" :value="s" />
      </datalist>

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

      <h3 class="section-title">{{ t('stats.todo', { n: todo.length }) }}</h3>
      <ul v-if="todo.length" class="todo">
        <li v-for="x in todo" :key="x.id">
          <div class="line">
            <i class="dot" :style="{ background: repoColor(slots[x.repo]) }" />
            <span class="muted repo" :title="x.repo">{{ repoName(x.repo) }}</span>
            <button class="link title" @click="emit('open-session', x.id)">{{ x.title || t('untitled') }}</button>
            <span class="chip" :class="x.status">{{ t(`status.${x.status}`) }}</span>
            <span class="muted num when">{{ when(x.last_ts) }}</span>
            <button class="btn small" @click="finish(x.id)">{{ t('detail.markDone') }}</button>
          </div>
          <p v-if="x.next" class="next">{{ x.next }}</p>
        </li>
      </ul>
      <p v-else class="muted">{{ t('stats.noTodo') }}</p>
      <p v-if="data.unsummarized" class="muted small">{{ t('stats.unsummarized', { n: data.unsummarized }) }}</p>

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
.cards{display:flex; gap:16px; margin-bottom:20px}
.card{display:flex; flex-direction:column; gap:2px; min-width:200px; padding:14px 18px; border:1px solid var(--rule); border-radius:12px}
.card .muted{font-size:13px}
.card b{font-size:28px; font-weight:400; color:var(--ink)}
.project td{font-weight:500; padding-top:16px}
.repo td:first-child{padding-left:28px; color:var(--ink-soft)}
.dot{display:inline-block; width:10px; height:10px; border-radius:50%; margin-right:8px}
.assign{width:10em; height:30px; border:1px solid var(--rule); border-radius:6px; background:var(--ground); padding:0 8px}
.assign:focus{border-color:var(--accent); outline:none}
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
