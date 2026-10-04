<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { getPins, getSchedules, getWeek, rescan } from './api.js'
import { addDays, startOfDay, weekStart } from './layout.js'
import { lang, locale, setLang, t } from './i18n.js'
import { CALENDAR_VIEWS, formatRoute, parseRoute } from './route.js'
import TimeGrid from './components/TimeGrid.vue'
import DayList from './components/DayList.vue'
import MonthView from './components/MonthView.vue'
import StatsView from './components/StatsView.vue'
import SessionDetail from './components/SessionDetail.vue'
import SettingsDialog from './components/SettingsDialog.vue'
import Sidebar from './components/Sidebar.vue'
import PlansView from './components/PlansView.vue'
import PlanForm from './components/PlanForm.vue'
import ReposView from './components/ReposView.vue'

// 画面の状態のうち、次に開いたときも残したいものはブラウザに覚える（保存できなければ既定に戻る）
function remembered(key, fallback, valid = () => true) {
  let v = fallback
  try {
    const raw = localStorage.getItem(key)
    if (raw != null) {
      const parsed = JSON.parse(raw)
      if (valid(parsed)) v = parsed
    }
  } catch {
    // 読めなければ既定
  }
  const r = ref(v)
  watch(
    r,
    (x) => {
      try {
        localStorage.setItem(key, JSON.stringify(x))
      } catch {
        // 保存できなくても画面は切り替わる
      }
    },
    { deep: true },
  )
  return r
}

const VIEWS = ['day', 'week', 'month', 'stats', 'plans', 'repos']
// 日付で動く表示（前へ・次へと今日のボタンを出す）
const CALENDAR = ['day', 'week', 'month', 'stats']
const view = remembered('agent-calendar.view3', 'week', (v) => VIEWS.includes(v))
const sidebarOpen = remembered('agent-calendar.sidebar', true, (v) => typeof v === 'boolean')
// 選んだホスト（null = すべて、'' = このマシン）と、隠したリポジトリ
const hosts = remembered('agent-calendar.hosts', null, (v) => v === null || Array.isArray(v))
const hiddenRepos = remembered('agent-calendar.hiddenRepos', [], Array.isArray)
// リポジトリの画面で選んでいる組織（URL に無ければ前回のもの）
const repoOwner = remembered('agent-calendar.repoOwner', null, (v) => v === null || typeof v === 'string')

// 表示の基準の日（その日の 0:00）。日・週・月の範囲はここから決める
const anchor = ref(startOfDay(Date.now()))
const monthStart = computed(() => {
  const d = new Date(anchor.value)
  return new Date(d.getFullYear(), d.getMonth(), 1).getTime()
})
const gridFrom = computed(() => weekStart(monthStart.value))
const span = computed(() => {
  if (view.value === 'day') return [anchor.value, addDays(anchor.value, 1)]
  if (view.value === 'month') return [gridFrom.value, addDays(gridFrom.value, 42)]
  const w = weekStart(anchor.value)
  return [w, addDays(w, 7)]
})
// サイドバーの「時間の分析情報」で集計する範囲。日・週は表示中の範囲、月（と集計・予定）はその月
const insightRange = computed(() => {
  if (view.value === 'day' || view.value === 'week') return span.value
  const d = new Date(monthStart.value)
  return [monthStart.value, new Date(d.getFullYear(), d.getMonth() + 1, 1).getTime()]
})
const from = computed(() => span.value[0])
const to = computed(() => span.value[1])
// ‹ › は表示に合わせて 1 日・1 週・1 か月ずつ
function step(n) {
  const d = new Date(anchor.value)
  if (view.value === 'day') anchor.value = addDays(anchor.value, n)
  else if (view.value === 'month') anchor.value = new Date(d.getFullYear(), d.getMonth() + n, 1).getTime()
  else anchor.value = addDays(anchor.value, 7 * n)
}
function openDay(d) {
  anchor.value = startOfDay(d)
  view.value = 'day'
}
const data = ref({ sessions: [], repos: {}, machines: [] })
const error = ref('')
const loading = ref(false)
const selectedId = ref(null)
const showSettings = ref(false)
// 詳細パネル: 会話・変更のタブでは広げる（変更は左に一覧・右に差分なので特に広く）。tab は開いているタブ、full は画面いっぱい（閉じたら戻す）。
// tab と full は URL にも持つ（リロードしても同じ状態で開く）
const detailFull = ref(false)
const detailTab = ref('summary')
const DETAIL_W = { conversation: '640px', changes: 'min(960px, 60vw)' }

// 集計・予定・リポジトリの画面と開いているセッションは URL に持たせる（戻る・進む・ブックマーク・共有ができるように）。
// カレンダーは URL を変えない（"/"）。"/" に戻ったら、最後に見ていた日・週・月の表示にする
const lastCalendar = remembered('agent-calendar.calendarView', 'week', (v) => CALENDAR_VIEWS.includes(v))
watch(view, (v) => {
  if (CALENDAR_VIEWS.includes(v)) lastCalendar.value = v
})
function applyRoute() {
  const r = parseRoute(location.pathname, location.search)
  if (r.view !== 'calendar') view.value = r.view
  else if (!CALENDAR_VIEWS.includes(view.value)) view.value = lastCalendar.value
  if (r.owner) repoOwner.value = r.owner
  selectedId.value = r.session
  detailTab.value = r.tab
  detailFull.value = r.full
}
applyRoute()
const currentUrl = computed(() =>
  formatRoute({
    view: view.value,
    owner: repoOwner.value,
    session: selectedId.value,
    tab: detailTab.value,
    full: detailFull.value,
  }),
)
history.replaceState(null, '', currentUrl.value)
// 表示が変わったら履歴に積む。戻る・進むで URL が変わったときは、その URL のとおりに表示する
// 組織が自動で決まっただけ（/repos → /repos/<組織>）のときは、履歴を増やさず置き換える
watch(currentUrl, (url, old) => {
  if (url === location.pathname + location.search) return
  if (old?.split('?')[0] === '/repos' && url.startsWith('/repos/')) history.replaceState(null, '', url)
  else history.pushState(null, '', url)
})
const onPopState = () => applyRoute()
onMounted(() => window.addEventListener('popstate', onPopState))
onUnmounted(() => window.removeEventListener('popstate', onPopState))
// 予定の登録の窓（どの画面からでも開ける）。undefined = 閉じている、null = 日時なし、数値 = カレンダーで押した日時
const creating = ref(undefined)
function planSaved() {
  creating.value = undefined
  editingPlan.value = null
  load() // カレンダーの予定と、予定の画面（version を見ている）を読み直す
}
// カレンダーの予定を押したら、その予定の編集を開く
const editingPlan = ref(null)
async function openPlan(id) {
  try {
    editingPlan.value = (await getSchedules()).schedules.find((p) => p.id === id) ?? null
  } catch (e) {
    error.value = String(e)
  }
}
// 予定はこのマシンで動くので、ホストの絞り込みで手元を外したときは出さない
const plans = computed(() => (hosts.value == null || hosts.value.includes('') ? (data.value.plans ?? []) : []))
watch(selectedId, (id) => {
  if (id) return
  detailFull.value = false
  detailTab.value = 'summary'
})
const now = ref(Date.now())
// 読み直した回数。集計とサイドバーはこれを見て取り直す（週の範囲が変わらなくても数字は変わる）
const version = ref(0)

// ピン留めした会話（サイドバーの上）
const pins = ref([])
async function loadPins() {
  try {
    pins.value = (await getPins()).pins
  } catch (e) {
    error.value = String(e)
  }
}

async function load() {
  error.value = ''
  loadPins()
  try {
    data.value = await getWeek(from.value, to.value)
    version.value++
  } catch (e) {
    error.value = String(e)
  }
}

async function reload() {
  loading.value = true
  try {
    await rescan()
    await load()
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

watch([from, to], load)
let timer
onMounted(() => {
  load()
  // 今の時刻の線を動かし、裏の scan で増えた分も拾う
  timer = setInterval(() => {
    now.value = Date.now()
    load()
  }, 5 * 60 * 1000)
})
onUnmounted(() => clearInterval(timer))

// 見出し: 日は「2026年10月4日(日)」、月は「2026年10月」、週は「2026年 9月 28日 – 10月 4日」
const range = computed(() => {
  if (view.value === 'day') {
    return new Date(anchor.value).toLocaleDateString(locale(), { year: 'numeric', month: 'long', day: 'numeric', weekday: 'short' })
  }
  if (view.value === 'month') return new Date(monthStart.value).toLocaleDateString(locale(), { year: 'numeric', month: 'long' })
  const fmt = new Intl.DateTimeFormat(locale(), { year: 'numeric', month: 'short', day: 'numeric' })
  return fmt.formatRange(new Date(from.value), new Date(addDays(from.value, 6)))
})

const machines = computed(() => data.value.machines ?? [])
const machineLabels = computed(() => Object.fromEntries(machines.value.map((m) => [m.id, m.label])))
const multiHost = computed(() => machines.value.length > 1)
// 一覧に無くなったホストの選択は捨てる（すべて外れたら「すべて」に戻す）
watch(machines, (list) => {
  if (hosts.value == null || !list.length) return
  const ids = new Set(list.map((m) => m.id))
  const kept = hosts.value.filter((h) => ids.has(h))
  if (kept.length !== hosts.value.length) hosts.value = kept.length ? kept : null
})
const hostSessions = computed(() =>
  hosts.value == null ? data.value.sessions : data.value.sessions.filter((s) => hosts.value.includes(s.machine)),
)
const sessions = computed(() => hostSessions.value.filter((s) => !hiddenRepos.value.includes(s.repo)))

// 選んだホストにこの期間の記録が無いとき、最後の記録の日を案内する（古い記録しか無いホストで「何も出ない」と迷わないように）
const emptyHints = computed(() => {
  // 「すべて」のときは、何も出ていない場合だけ案内する（ほかのホストの記録が出ているなら邪魔になる）
  if (view.value === 'stats' || view.value === 'plans' || (hosts.value == null && hostSessions.value.length)) return []
  const chosen = machines.value.filter((m) => hosts.value == null || hosts.value.includes(m.id))
  return chosen
    .filter((m) => !hostSessions.value.some((s) => s.machine === m.id))
    .map((m) => ({ ...m, when: m.last_ts ? new Date(m.last_ts).toLocaleDateString(locale(), { year: 'numeric', month: 'short', day: 'numeric' }) : null }))
})

// サイドバーのリポジトリ一覧。選んだホストの分で数え、多い順に
const repoList = computed(() => {
  const counts = new Map()
  for (const s of hostSessions.value) counts.set(s.repo, (counts.get(s.repo) ?? 0) + 1)
  return [...counts]
    .map(([repo, count]) => ({ repo, ...data.value.repos[repo], count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
})
// ホストごとの件数（表示中の期間。ホストの絞り込みには依らない）
const hostCounts = computed(() => {
  const counts = {}
  for (const s of data.value.sessions) counts[s.machine] = (counts[s.machine] ?? 0) + 1
  return counts
})

// ロゴを押したらカレンダーの今日へ戻る（集計・予定・リポジトリからは週の表示へ）
function backToCalendar() {
  if (!['day', 'week', 'month'].includes(view.value)) view.value = 'week'
  anchor.value = startOfDay(Date.now())
  detailFull.value = false
}

// キーボードのショートカット（Google カレンダーと同じ）。入力中・修飾キー付きでは反応しない
const KEYS = { d: 'day', w: 'week', m: 'month', s: 'stats', a: 'plans', r: 'repos' }
const SHORTCUT = Object.fromEntries(Object.entries(KEYS).map(([k, v]) => [v, k]))
function onKey(e) {
  if (e.ctrlKey || e.metaKey || e.altKey || e.isComposing) return
  if (e.target.closest?.('input, select, textarea, [contenteditable]')) return
  const k = e.key.toLowerCase()
  if (KEYS[k]) view.value = KEYS[k]
  else if (k === 't') anchor.value = startOfDay(Date.now())
  else if (k === 'j' || k === 'n') step(1)
  else if (k === 'k' || k === 'p') step(-1)
  else return
  e.preventDefault()
}
onMounted(() => window.addEventListener('keydown', onKey))
onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="app" :class="{ withDetail: selectedId, withSidebar: sidebarOpen, detailFull: selectedId && detailFull }" :style="{ '--detail-w': DETAIL_W[detailTab] ?? '440px' }" :lang="lang">
    <header class="top">
      <button class="icon-btn" :aria-label="t('sidebar.menu')" :aria-expanded="sidebarOpen" @click="sidebarOpen = !sidebarOpen">☰</button>
      <button class="brand" :title="t('backToCalendar')" @click="backToCalendar">
        <span class="logo" aria-hidden="true"><i /><i /><i /></span>
        <span class="name">{{ t('appName') }}</span>
      </button>
      <template v-if="CALENDAR.includes(view)">
      <button class="btn" :title="`${t('today')} (t)`" aria-keyshortcuts="t" @click="anchor = startOfDay(Date.now())">{{ t('today') }}</button>
      <div class="arrows">
        <button class="icon-btn" :aria-label="t('prev')" :title="`${t('prev')} (k)`" aria-keyshortcuts="k p" @click="step(-1)">‹</button>
        <button class="icon-btn" :aria-label="t('next')" :title="`${t('next')} (j)`" aria-keyshortcuts="j n" @click="step(1)">›</button>
      </div>
      <h1 class="range num">{{ range }}</h1>
      </template>
      <span class="spacer" />
      <span v-if="error" class="chip warn">{{ error }}</span>
      <div class="segmented" role="tablist">
        <button
          v-for="v in VIEWS"
          :key="v"
          role="tab"
          :aria-selected="view === v"
          :aria-keyshortcuts="SHORTCUT[v]"
          :title="`${t(`views.${v}`)} (${SHORTCUT[v]})`"
          :class="{ on: view === v }"
          @click="view = v"
        >
          {{ t(`views.${v}`) }}
        </button>
      </div>
      <div class="segmented small" role="group" aria-label="Language">
        <button :class="{ on: lang === 'en' }" @click="setLang('en')">EN</button>
        <button :class="{ on: lang === 'ja' }" @click="setLang('ja')">日本語</button>
      </div>
      <button class="icon-btn" :title="t('reload')" :aria-label="t('reload')" :disabled="loading" @click="reload">
        <span :class="{ spin: loading }">↻</span>
      </button>
      <button class="icon-btn" :title="t('settings.title')" :aria-label="t('settings.title')" @click="showSettings = true">⚙</button>
    </header>

    <main class="main">
      <Sidebar
        v-if="sidebarOpen"
        :anchor="anchor"
        :highlight="view === 'day' || view === 'week' ? span : null"
        :insight-range="insightRange"
        :now="now"
        :version="version"
        :machines="machines"
        :repos="repoList"
        :host-counts="hostCounts"
        v-model:hosts="hosts"
        v-model:hidden-repos="hiddenRepos"
        @pick="(d) => (anchor = startOfDay(d))"
        :pins="pins"
        :selected-id="selectedId"
        @more="view = 'stats'"
        @create="creating = null"
        @select="(id) => (selectedId = id)"
      />
      <div class="content" :class="{ fixed: !['stats', 'plans', 'repos'].includes(view) }">
        <div v-for="h in CALENDAR.includes(view) ? emptyHints : []" :key="h.id" class="hint">
          <span v-if="h.when">{{ t('emptyHost', { host: h.label, date: h.when }) }}</span>
          <span v-else>{{ t('neverHost', { host: h.label }) }}</span>
          <button v-if="h.when" class="link" @click="anchor = startOfDay(h.last_ts)">{{ t('jumpThere') }}</button>
        </div>
        <div v-if="view === 'day'" class="daygrid">
          <TimeGrid
            :from="from"
            :days="1"
            @create-at="(ms) => (creating = ms)"
            :sessions="sessions"
            :repos="data.repos"
            :machine-labels="machineLabels"
            :selected-id="selectedId"
            :now="now"
            :plans="plans"
            @select="(id) => (selectedId = id)"
            @open-plan="openPlan"
          />
          <DayList
            :from="from"
            :sessions="sessions"
            :repos="data.repos"
            :machine-labels="multiHost ? machineLabels : null"
            :selected-id="selectedId"
            @select="(id) => (selectedId = id)"
          />
        </div>
        <TimeGrid
          v-else-if="view === 'week'"
          class="fill"
          @create-at="(ms) => (creating = ms)"
          :from="from"
          :sessions="sessions"
          :repos="data.repos"
          :machine-labels="machineLabels"
          :selected-id="selectedId"
          :now="now"
          :plans="plans"
          @select="(id) => (selectedId = id)"
          @open-plan="openPlan"
        />
        <MonthView
          v-else-if="view === 'month'"
          class="fill"
          :month="monthStart"
          :grid-from="gridFrom"
          :sessions="sessions"
          :repos="data.repos"
          :machine-labels="multiHost ? machineLabels : null"
          :selected-id="selectedId"
          :now="now"
          :plans="plans"
          @select="(id) => (selectedId = id)"
          @open-day="openDay"
          @open-plan="openPlan"
        />
        <PlansView v-else-if="view === 'plans'" :version="version" @open-session="(id) => (selectedId = id)" @changed="load" />
        <ReposView
          v-else-if="view === 'repos'"
          v-model:owner="repoOwner"
          :version="version"
          :colors="data.repos"
          @open-session="(id) => (selectedId = id)"
          @open-settings="showSettings = true"
          @changed="load"
        />
        <StatsView v-else :from="weekStart(anchor)" :version="version" :hosts="hosts" />
      </div>
      <SessionDetail
        v-if="selectedId"
        :id="selectedId"
        :repos="data.repos"
        :machine-labels="machineLabels"
        class="side"
        @close="selectedId = null"
        v-model:tab="detailTab"
        :full="detailFull"
        @full="(f) => (detailFull = f)"
        @pinned="loadPins"
      />
    </main>
    <SettingsDialog v-if="showSettings" @close="showSettings = false" @saved="load" />
    <PlanForm v-if="creating !== undefined" :at="creating" @close="creating = undefined" @saved="planSaved" />
    <PlanForm v-if="editingPlan" :plan="editingPlan" @close="editingPlan = null" @saved="planSaved" />
  </div>
</template>

<style scoped>
.app{display:flex; flex-direction:column; height:100%}
.top{display:flex; align-items:center; gap:8px; flex-wrap:wrap; padding:8px 16px 8px 8px; border-bottom:1px solid var(--rule); min-height:64px}
.brand{display:flex; align-items:center; gap:10px; margin-right:24px; padding:4px 8px 4px 4px; border:0; border-radius:8px; background:none; color:inherit; font:inherit; cursor:pointer}
.brand:hover{background:var(--hover)}
.logo{display:grid; grid-template-columns:repeat(3, 6px); gap:2px; padding:6px; border:2px solid var(--accent); border-radius:6px}
.logo i{height:6px; border-radius:1px; background:var(--accent)}
.logo i:nth-child(2){background:#d50000}
.logo i:nth-child(3){background:#0b8043}
.name{font-size:22px; color:var(--ink-soft); font-weight:400}
.arrows{display:flex}
.range{font-size:22px; font-weight:400; margin:0 0 0 8px; color:var(--ink)}
.spacer{flex:1}
.segmented{display:inline-flex; border:1px solid var(--outline); border-radius:18px; overflow:hidden}
.segmented button{border:0; background:none; height:34px; padding:0 14px; color:var(--ink-soft); font-weight:500}
.segmented button + button{border-left:1px solid var(--outline)}
.segmented button.on{background:var(--accent-soft); color:#041e49}
.segmented.small button{height:30px; padding:0 10px; font-size:12px}
.spin{display:inline-block; animation:spin 1s linear infinite}
@keyframes spin{to{transform:rotate(360deg)}}

.main{flex:1; min-height:0; display:grid; grid-template-columns:1fr}
.app.withSidebar .main{grid-template-columns:256px 1fr}
.app.withDetail .main{grid-template-columns:1fr var(--detail-w)}
.app.withSidebar.withDetail .main{grid-template-columns:256px 1fr var(--detail-w)}
/* 詳細を画面いっぱいに広げたら、サイドバーとカレンダーは隠す */
.app.withDetail.detailFull .main,.app.withSidebar.withDetail.detailFull .main{grid-template-columns:1fr; grid-template-rows:1fr}
.app.detailFull .main > :not(.side){display:none}
.content{overflow-y:auto; min-height:0; padding:8px 16px 40px}
/* カレンダーは中で縦にスクロールするので、外は画面の高さに収める */
.content.fixed{overflow:hidden; display:flex; flex-direction:column; padding-bottom:8px}
.content.fixed .fill{flex:1; min-height:0}
.daygrid{flex:1; min-height:0; display:grid; grid-template-columns:minmax(0, 1fr) minmax(320px, 42%)}
.side{min-height:0}
.hint{display:flex; align-items:center; gap:12px; margin:4px 0 8px; padding:8px 14px; border-radius:8px; background:var(--accent-faint); font-size:13px}
.link{border:0; background:none; padding:0; color:var(--accent); font-weight:500; text-decoration:underline; text-underline-offset:.2em}

/* 狭い画面では、サイドバーを消さずに本文の上へ重ねる（☰ で開け閉めできるように） */
@media (max-width: 1100px){
  .app.withSidebar.withDetail .main{grid-template-columns:1fr 400px}
  .app.withSidebar.withDetail .main :deep(.sidebar){position:fixed; top:64px; left:0; bottom:0; z-index:15; background:var(--ground);
    box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
}
@media (max-width: 1000px){
  .app.withDetail .main,.app.withSidebar.withDetail .main{grid-template-columns:1fr; grid-template-rows:1fr 1fr}
  .app.withSidebar .main{grid-template-columns:1fr}
  .app.withSidebar .main :deep(.sidebar){position:fixed; top:64px; left:0; bottom:0; z-index:15; background:var(--ground);
    box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
  .side{border-top:1px solid var(--rule)}
  .name{display:none}
}
</style>
