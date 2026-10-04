<script setup>
import { computed, ref, watch } from 'vue'
import { cloneRepo, getRepoSessions, getRepos, rescan } from '../api.js'
import { locale, t } from '../i18n.js'
import { DAY_MS } from '../layout.js'
import { repoColor } from '../colors.js'
import StartDialog from './StartDialog.vue'

// リポジトリの画面: 先に組織を選び、その組織の GitHub のリポジトリを表で並べる（手元にあるものを上、無いものはたたむ）。
// 手元にあるものはそこで新しいセッションを始められ、無いものは探すフォルダへ clone できる
const props = defineProps({
  version: { type: Number, required: true }, // 読み直すたびに増える
  colors: { type: Object, default: () => ({}) }, // リポジトリのパス → { slot }（いまカレンダーに出ている色）
  owner: { type: String, default: null }, // 選んでいる組織（URL に持つので App が覚える）
})
const emit = defineEmits(['open-session', 'open-settings', 'changed', 'update:owner'])

const data = ref(null)
const error = ref('')
const loading = ref(false)
const query = ref('')
const cloneRoot = ref('')
const cloning = ref(null) // clone 中のリポジトリ（owner/name）
const starting = ref(null) // 「ここで始める」を開いているリポジトリ

const owner = computed({ get: () => props.owner, set: (o) => emit('update:owner', o) })

async function load(refresh = false) {
  loading.value = true
  try {
    data.value = await getRepos(refresh)
    error.value = ''
    if (!data.value.roots.includes(cloneRoot.value)) cloneRoot.value = data.value.roots[0] ?? ''
    if (!data.value.owners.includes(owner.value)) owner.value = data.value.owners[0] ?? null
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    loading.value = false
  }
}
watch(() => props.version, () => load(), { immediate: true })

// 選んでいる組織のリポジトリを、手元にあるものと無いものに分ける。どちらも最近動いたものから
const group = computed(() => (data.value?.groups ?? []).find((g) => g.owner === owner.value) ?? null)
const recent = (r) => Math.max(r.last_ts ?? 0, Date.parse(r.pushed_at) || 0)
const shown = computed(() => {
  const q = query.value.trim().toLowerCase()
  const repos = (group.value?.repos ?? [])
    .filter((r) => !q || `${r.name} ${r.description}`.toLowerCase().includes(q))
    .sort((a, b) => recent(b) - recent(a))
  return { local: repos.filter((r) => r.local), remote: repos.filter((r) => !r.local) }
})
const counts = computed(() => Object.fromEntries((data.value?.groups ?? []).map((g) => [g.owner, g.repos.length])))
const ready = computed(() => data.value && data.value.owners.length && data.value.roots.length)
const day = (ms) => new Date(ms).toLocaleDateString(locale(), { year: 'numeric', month: 'numeric', day: 'numeric' })
// カレンダーに出ているリポジトリは同じ色の点、出ていないものは灰色の輪
const dot = (r) => {
  const info = r.local ? props.colors[r.local] : null
  return info ? { background: repoColor(info.slot), borderColor: repoColor(info.slot) } : null
}
// 7 日以内の作業・更新は青く強める
const fresh = (ms) => ms && Date.now() - ms < 7 * DAY_MS
// そのリポジトリの過去のセッション。押した行の下に開く（一度に 1 つ）
const openRepo = ref(null) // 開いているリポジトリのパス
const repoSessions = ref([])
const sessionsError = ref('')
async function toggleSessions(r) {
  if (openRepo.value === r.local) {
    openRepo.value = null
    return
  }
  openRepo.value = r.local
  repoSessions.value = []
  sessionsError.value = ''
  try {
    const list = (await getRepoSessions(r.local)).sessions
    if (openRepo.value === r.local) repoSessions.value = list
  } catch (e) {
    sessionsError.value = String(e.message ?? e)
  }
}
const when = (s) =>
  new Intl.DateTimeFormat(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' }).formatRange(
    new Date(s.first_ts),
    new Date(s.last_ts),
  )
// 手元に無いものは、絞り込み中でなければたたんでおく
const showRemote = ref(false)

async function clone(r) {
  const key = `${r.owner}/${r.name}`
  cloning.value = key
  error.value = ''
  try {
    await cloneRepo(r.owner, r.name, cloneRoot.value)
    await load()
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    cloning.value = null
  }
}

// できたセッションを開く。終わった直後はまだ読み直しが済んでいないことがあるので、先に読み直す
async function opened(id) {
  starting.value = null
  try {
    await rescan()
  } catch (e) {
    error.value = String(e.message ?? e)
  }
  emit('changed')
  emit('open-session', id)
}
</script>

<template>
  <div class="repos">
    <div v-if="data?.owners.length" class="owners" role="tablist">
      <button
        v-for="o in data.owners"
        :key="o"
        role="tab"
        :aria-selected="owner === o"
        :class="{ on: owner === o }"
        @click="owner = o"
      >
        {{ o }} <span class="muted num">{{ counts[o] ?? 0 }}</span>
      </button>
    </div>
    <div class="bar">
      <input v-model="query" class="filter" type="search" :placeholder="t('repos.filter')" :aria-label="t('repos.filter')" />
      <label v-if="data?.roots.length > 1" class="root">
        {{ t('repos.cloneTo') }}
        <select v-model="cloneRoot">
          <option v-for="r in data.roots" :key="r" :value="r">{{ r }}</option>
        </select>
      </label>
      <span class="spacer" />
      <button class="btn" :disabled="loading" @click="load(true)">{{ t('repos.refresh') }}</button>
    </div>
    <p v-if="error" class="chip warn">{{ error }}</p>
    <p v-if="loading" class="muted">{{ t('repos.loading') }}</p>
    <div v-if="data && !ready" class="setup">
      <p class="muted">{{ t('repos.setup') }}</p>
      <button class="btn primary" @click="emit('open-settings')">{{ t('repos.openSettings') }}</button>
    </div>

    <template v-if="group">
      <p v-if="group.error" class="chip warn">{{ t('repos.failed', { owner: group.owner, e: group.error }) }}</p>
      <p v-else-if="!group.repos.length" class="muted">{{ t('repos.empty') }}</p>
      <template v-else>
        <h3 class="sec">{{ t('repos.onMachine') }} <span class="muted num">{{ shown.local.length }}</span></h3>
        <div v-if="shown.local.length" class="table">
          <div class="row head">
            <span>{{ t('repos.colRepo') }}</span>
            <span>{{ t('repos.colPushed') }}</span>
            <span />
          </div>
          <template v-for="r in shown.local" :key="r.name">
          <div class="row" :class="{ archived: r.archived, open: openRepo === r.local }">
            <span class="cell-main">
              <span class="title">
                <i class="dot" :style="dot(r)" />
                <a :href="r.url" target="_blank" rel="noopener noreferrer" :title="r.local">{{ r.name }}</a>
                <span v-if="r.private" class="tag private">{{ t('repos.private') }}</span>
                <span v-if="r.archived" class="tag">{{ t('repos.archived') }}</span>
              </span>
              <span v-if="r.description" class="desc">{{ r.description }}</span>
            </span>
            <a v-if="r.pushed_at" class="num date link" :class="{ fresh: fresh(Date.parse(r.pushed_at)) }" :href="`${r.url}/commits`" target="_blank" rel="noopener noreferrer" :title="t('repos.commits')">{{ day(Date.parse(r.pushed_at)) }}</a>
            <span v-else class="muted">—</span>
            <span class="acts">
              <button v-if="r.sessions" class="link-btn" :aria-expanded="openRepo === r.local" @click="toggleSessions(r)">
                {{ t('repos.sessions', { n: r.sessions }) }} {{ openRepo === r.local ? '▾' : '▸' }}
              </button>
              <button class="btn small accent" @click="starting = r">{{ t('repos.start') }}</button>
            </span>
          </div>
          <div v-if="openRepo === r.local" class="sessions">
            <p v-if="sessionsError" class="chip warn">{{ sessionsError }}</p>
            <button v-for="x in repoSessions" :key="x.id" class="session" @click="emit('open-session', x.id)">
              <span class="when num" :class="{ fresh: fresh(x.last_ts) }">{{ when(x) }}</span>
              <span class="stitle">{{ x.title || t('untitled') }}</span>
              <span v-if="x.source === 'codex'" class="chip src">{{ t('source.codex') }}</span>
              <span v-if="x.status" class="chip" :class="x.status">{{ t(`status.${x.status}`) }}</span>
            </button>
          </div>
          </template>
        </div>

        <h3 class="sec">
          <button class="fold" :aria-expanded="showRemote || !!query" @click="showRemote = !showRemote">
            {{ showRemote || query ? '▾' : '▸' }} {{ t('repos.notCloned') }} <span class="muted num">{{ shown.remote.length }}</span>
          </button>
        </h3>
        <div v-if="(showRemote || query) && shown.remote.length" class="table">
          <div v-for="r in shown.remote" :key="r.name" class="row remote" :class="{ archived: r.archived }">
            <span class="cell-main">
              <span class="title">
                <i class="dot" />
                <a :href="r.url" target="_blank" rel="noopener noreferrer">{{ r.name }}</a>
                <span v-if="r.private" class="tag private">{{ t('repos.private') }}</span>
                <span v-if="r.archived" class="tag">{{ t('repos.archived') }}</span>
              </span>
              <span v-if="r.description" class="desc">{{ r.description }}</span>
            </span>
            <a v-if="r.pushed_at" class="num date link" :class="{ fresh: fresh(Date.parse(r.pushed_at)) }" :href="`${r.url}/commits`" target="_blank" rel="noopener noreferrer" :title="t('repos.commits')">{{ day(Date.parse(r.pushed_at)) }}</a>
            <span v-else class="muted">—</span>
            <button class="btn small" :disabled="!cloneRoot || cloning !== null" @click="clone(r)">
              {{ cloning === `${r.owner}/${r.name}` ? t('repos.cloning') : t('repos.clone') }}
            </button>
          </div>
        </div>
      </template>
    </template>
    <StartDialog v-if="starting" :repo="starting" @close="starting = null" @opened="opened" @ran="emit('changed')" />
  </div>
</template>

<style scoped>
.repos{max-width:960px}
.owners{display:flex; gap:4px; flex-wrap:wrap; border-bottom:1px solid var(--rule); margin:4px 0 12px}
.owners button{border:0; background:none; padding:8px 16px; color:var(--muted); font-weight:500; border-bottom:3px solid transparent; margin-bottom:-1px}
.owners button.on{color:var(--accent); border-bottom-color:var(--accent)}
.owners button .num{font-weight:400; margin-left:4px}
.bar{display:flex; align-items:center; gap:12px; margin:4px 0 12px; flex-wrap:wrap}
.filter{height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 10px; font:inherit; min-width:220px}
.root{display:flex; align-items:center; gap:6px; font-size:13px; color:var(--ink-soft)}
.root select{height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 8px; background:var(--ground)}
.spacer{flex:1}
.setup{display:flex; align-items:center; gap:12px; margin:16px 0}
.sec{display:flex; align-items:center; gap:6px; font-size:14px; font-weight:500; margin:20px 0 8px; color:var(--ink)}
.fold{border:0; background:none; padding:0; font:inherit; color:inherit; cursor:pointer}
.table{border:1px solid var(--rule); border-radius:12px; overflow:hidden}
.row{display:grid; grid-template-columns:minmax(0, 1fr) 110px 260px; gap:16px; align-items:center; padding:10px 16px; border-top:1px solid var(--rule-soft)}
.row:first-child{border-top:0}
.row.open{background:var(--accent-faint)}
.row:hover:not(.head){background:var(--hover)}
.row.head{padding:8px 16px; font-size:12px; color:var(--muted); background:var(--panel)}
.row > :last-child{justify-self:end}
.row.archived .title a{color:var(--muted)}
.cell-main{display:flex; flex-direction:column; min-width:0}
.title{display:flex; align-items:center; gap:8px; min-width:0}
.title a{font-weight:500; color:var(--ink); text-decoration:none; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.title a:hover{text-decoration:underline}
.tag{flex:none; font-size:11px; color:var(--muted); border:1px solid var(--rule); border-radius:4px; padding:0 5px}
.tag.private{color:var(--talk); border-color:var(--talk)}
.dot{flex:none; width:10px; height:10px; border-radius:50%; border:2px solid var(--rule); box-sizing:border-box}
.date.fresh{color:var(--accent); font-weight:500}
.btn.accent{color:var(--accent); border-color:var(--accent)}
.btn.accent:hover{background:var(--accent-faint)}
.row.remote .title a{color:var(--ink-soft); font-weight:400}
.desc{font-size:12px; color:var(--muted); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; margin-top:2px; padding-left:18px}
.date{font-size:13px; color:var(--ink-soft)}
.date.link{text-decoration:none}
.date.link:hover{color:var(--accent); text-decoration:underline}
.btn.small{height:30px; padding:0 12px; font-size:13px}
.acts{display:flex; align-items:center; gap:12px}
.link-btn{border:0; background:none; padding:0; font:inherit; font-size:13px; color:var(--accent); cursor:pointer}
.link-btn:hover{text-decoration:underline}
.sessions{border-top:1px solid var(--rule-soft); padding:4px 16px 8px 34px; max-height:360px; overflow-y:auto}
.session{display:flex; align-items:center; gap:10px; width:100%; border:0; background:none; padding:6px 8px; border-radius:8px; text-align:left; min-width:0}
.session:hover{background:var(--hover)}
.session .when{flex:none; width:190px; font-size:12px; color:var(--ink-soft)}
.session .when.fresh{color:var(--accent); font-weight:500}
.session .stitle{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px}
</style>
