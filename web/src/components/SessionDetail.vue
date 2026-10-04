<script setup>
import { computed, onUnmounted, ref, watch } from 'vue'
import { getSession, resumeSession, setPin, summarizeSession } from '../api.js'
import { fmtDuration, fmtTokens, fmtUsd } from '../layout.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'
import Conversation from './Conversation.vue'
import Changes from './Changes.vue'

// 1 セッションの詳細。Google カレンダーの予定の詳細と同じく、色の四角・題・日時を上に置く
const props = defineProps({
  id: { type: String, required: true },
  repos: { type: Object, required: true },
  machineLabels: { type: Object, default: () => ({}) },
  full: { type: Boolean, default: false }, // 画面いっぱいに広げている
  tab: { type: String, default: 'summary' }, // 開いているタブ（URL に持つので App が覚える）
})
const emit = defineEmits(['close', 'full', 'pinned', 'update:tab'])

// 概要・会話・変更のタブ（URL に持つので App が覚える。パネルの幅も App がタブで決める）
const tab = computed({ get: () => props.tab, set: (v) => emit('update:tab', v) })

const s = ref(null)
const error = ref('')
// 前に押したセッションの応答が後から届いても、今選んでいるものでなければ捨てる
async function load(id) {
  try {
    const r = await getSession(id)
    if (id === props.id) s.value = r
  } catch (e) {
    if (id === props.id) error.value = String(e)
  }
}

const repo = computed(() => (s.value && props.repos[s.value.repo]) ?? null)
const repoName = computed(() => repo.value?.name ?? s.value?.repo.split('/').pop())
const when = computed(() => {
  if (!s.value) return ''
  const fmt = new Intl.DateTimeFormat(locale(), { month: 'short', day: 'numeric', weekday: 'short', hour: 'numeric', minute: '2-digit' })
  return fmt.formatRange(new Date(s.value.first_ts), new Date(s.value.last_ts))
})
const clock = (ms) => new Date(ms).toLocaleTimeString(locale(), { hour: 'numeric', minute: '2-digit' })

const costRows = computed(() =>
  ['main', 'subagent', 'advisor'].filter((k) => s.value?.cost.by_kind[k]).map((k) => ({ k, ...s.value.cost.by_kind[k] })),
)
const allTokens = (x) => x.input + x.output + x.cache_read + x.cache_5m + x.cache_1h

// 実行中かどうか（claude agents が答えなかったとき・Codex では出さない）
const running = computed(() => (s.value?.running.known ? s.value.running.kind ?? null : undefined))
// Remote Control で再開できるのは、このマシンの Claude Code のセッションだけ
const isClaude = computed(() => s.value?.source === 'claude' && !s.value?.machine)

const resume = ref(null) // { status, url } | { error }
const resuming = ref(false)
// claude.ai で続ける: 裏で Remote Control つきで再開し、つなぎ先を新しいタブで開く。
// 待ってから開くとポップアップとして止められるので、先に空のタブを開いておく
async function doResume() {
  const id = s.value.id // 表示しているセッションを起動する
  const tabWin = window.open('about:blank', '_blank')
  resuming.value = true
  try {
    const r = await resumeSession(id)
    if (r.url && tabWin) tabWin.location = r.url
    else tabWin?.close()
    if (id === props.id) resume.value = r
    await load(id)
  } catch (e) {
    tabWin?.close()
    if (id === props.id) resume.value = { error: String(e.message ?? e) }
  } finally {
    resuming.value = false
  }
}

// ピン留め（サイドバーの上に並べる）。付け外ししたら App に知らせて一覧を読み直す
async function togglePin() {
  const id = s.value.id
  const on = !s.value.pinned
  try {
    await setPin(id, on)
    if (id === props.id) s.value.pinned = on
    emit('pinned')
  } catch (e) {
    if (id === props.id) error.value = String(e.message ?? e)
  }
}

// 今すぐ要約する。要約には 1 分ほどかかるので、どのセッションを要約中かで押せる・押せないを分ける
const summarizingId = ref(null)
const summaryError = ref('')
async function doSummarize() {
  const id = s.value.id
  summarizingId.value = id
  summaryError.value = ''
  try {
    await summarizeSession(id)
    await load(id)
  } catch (e) {
    if (id === props.id) summaryError.value = String(e.message ?? e)
  } finally {
    summarizingId.value = null
  }
}

// 別のタブから指示を実行中のあいだは状態を見に行き、終わったら入力欄を戻して会話を読み直す
const convKey = ref(0)
let poll = null
watch(
  () => s.value?.sending,
  (now, before) => {
    clearInterval(poll)
    poll = null
    if (now) poll = setInterval(() => load(props.id), 4000)
    else if (before) convKey.value++
  },
)
onUnmounted(() => clearInterval(poll))

// 会話から送れないときの理由
const blocked = computed(() => {
  if (!s.value) return null
  if (s.value.machine) return t('conv.noRemote', { machine: props.machineLabels[s.value.machine] ?? s.value.machine })
  if (running.value === 'interactive' || running.value === 'background') return t('conv.noRunning')
  if (s.value.sending) return t('conv.busy')
  return null
})
const copied = ref(false)
async function copy() {
  try {
    await navigator.clipboard.writeText(s.value.resume_command)
    copied.value = true
  } catch (e) {
    error.value = t('detail.copyFailed', { e })
  }
}

// 選び直したら前のセッションの再開結果・コピー済みの表示を消す（ref の宣言より後に置く）
watch(
  () => props.id,
  (id) => {
    s.value = null
    error.value = ''
    resume.value = null
    copied.value = false
    summaryError.value = ''
    load(id)
  },
  { immediate: true },
)
</script>

<template>
  <aside class="detail" :class="{ full, changes: tab === 'changes' }">
    <div class="tools">
      <button
        v-if="s"
        class="icon-btn pin"
        :class="{ on: s.pinned }"
        :aria-label="s.pinned ? t('detail.unpin') : t('detail.pin')"
        :title="s.pinned ? t('detail.unpin') : t('detail.pin')"
        :aria-pressed="s.pinned"
        @click="togglePin"
      >
        📌
      </button>
      <button
        class="icon-btn"
        :aria-label="full ? t('detail.shrink') : t('detail.expand')"
        :title="full ? t('detail.shrink') : t('detail.expand')"
        :aria-pressed="full"
        @click="emit('full', !full)"
      >
        {{ full ? '⤡' : '⤢' }}
      </button>
      <button class="icon-btn" :aria-label="t('close')" @click="emit('close')">✕</button>
    </div>
    <p v-if="error" class="chip warn">{{ error }}</p>
    <p v-if="!s && !error" class="muted">{{ t('detail.loading') }}</p>
    <template v-if="s">
      <header class="head">
        <i class="square" :style="{ background: repoColor(repo?.slot) }" />
        <div>
          <h2>{{ s.summary?.title || s.title || t('untitled') }}</h2>
          <div class="when num">{{ when }}</div>
          <div class="meta muted">
            <span>{{ repoName }}</span>
            <code v-if="s.branch">{{ s.branch }}</code>
            <span class="chip src">{{ t(`source.${s.source}`) }}</span>
            <span v-if="s.machine || Object.keys(machineLabels).length > 1" class="chip src">
              {{ t('detail.onMachine', { machine: machineLabels[s.machine] ?? s.machine }) }}
            </span>
            <span v-if="running === 'interactive'" class="chip wip">{{ t('detail.runningTerminal') }}</span>
            <span v-else-if="running === 'background'" class="chip wip">{{ t('detail.runningBackground') }}</span>
            <span v-if="s.github" class="gh">
              <a :href="s.github.repo" target="_blank" rel="noopener noreferrer">GitHub ↗</a>
              <a v-if="s.github.branch" :href="s.github.branch" target="_blank" rel="noopener noreferrer">{{ t('detail.ghBranch') }} ↗</a>
              <a v-if="s.github.compare" :href="s.github.compare" target="_blank" rel="noopener noreferrer">{{ t('detail.ghCompare') }} ↗</a>
            </span>
          </div>
          <div class="meta muted">
            {{ t('detail.openFor', { d: fmtDuration(s.last_ts - s.first_ts) }) }} · {{ t('detail.prompts', { n: s.prompt_count }) }}
          </div>
        </div>
      </header>

      <div class="actions">
        <button v-if="isClaude" class="btn primary" :disabled="resuming || running === 'interactive'" @click="doResume">
          {{ resuming ? t('detail.resuming') : running === 'background' ? t('detail.resumeOpen') : t('detail.resume') }}
        </button>
        <button class="btn" @click="copy">{{ copied ? t('detail.copied') : t('detail.copy') }}</button>
      </div>
      <!-- 端末で開いているセッションは裏で重ねて開かない（同じ記録に 2 か所から書く）。端末の /remote-control なら続けられる -->
      <p v-if="isClaude && running === 'interactive' && !resume" class="muted small">{{ t('detail.openInTerminal') }}</p>
      <div v-if="resume" class="resume">
        <span v-if="resume.error && /open in a terminal/.test(resume.error)" class="muted small">{{ t('detail.openInTerminal') }}</span>
        <span v-else-if="resume.error" class="chip warn">{{ resume.error }}</span>
        <template v-else>
          <span class="chip wip">{{ resume.status === 'running' ? t('detail.alreadyRunning') : t('detail.started') }}</span>
          <a v-if="resume.url" :href="resume.url" target="_blank" rel="noopener">{{ t('detail.openInClaude') }}</a>
          <span v-else class="muted">{{ t('detail.findOnPhone') }}</span>
        </template>
      </div>

      <nav class="tabs" role="tablist">
        <button role="tab" :aria-selected="tab === 'summary'" :class="{ on: tab === 'summary' }" @click="tab = 'summary'">
          {{ t('detail.tabs.summary') }}
        </button>
        <button role="tab" :aria-selected="tab === 'conversation'" :class="{ on: tab === 'conversation' }" @click="tab = 'conversation'">
          {{ t('detail.tabs.conversation') }}
        </button>
        <button v-if="!s.machine" role="tab" :aria-selected="tab === 'changes'" :class="{ on: tab === 'changes' }" @click="tab = 'changes'">
          {{ t('detail.tabs.changes') }}
        </button>
      </nav>

      <Changes v-if="tab === 'changes' && !s.machine" :id="s.id" :commits="s.commits" :github="s.github" class="conv" />
      <Conversation
        v-else-if="tab === 'conversation'"
        :id="s.id"
        :key="convKey"
        :blocked="blocked"
        :following="!!s.sending"
        class="conv"
        @sent="load(s.id)"
      />
      <template v-else>
      <h3 class="section-title">
        {{ t('detail.summary') }}
        <span v-if="s.summary?.status" class="chip" :class="s.summary.status">{{ t(`status.${s.summary.status}`) }}</span>
        <span class="spacer" />
        <button class="btn small" :disabled="summarizingId !== null" @click="doSummarize">
          {{ summarizingId === s.id ? t('detail.summarizing') : s.summary ? t('detail.resummarize') : t('detail.summarizeNow') }}
        </button>
      </h3>
      <p v-if="summaryError" class="chip warn">{{ summaryError }}</p>
      <template v-if="s.summary">
        <ul class="bullets">
          <li v-for="(b, i) in s.summary.bullets" :key="i">{{ b }}</li>
        </ul>
        <div v-if="s.summary.next" class="note"><span class="title">{{ t('detail.next') }}</span><br />{{ s.summary.next }}</div>
        <p class="muted small">
          {{ t('detail.summarizedBy', { model: s.summary.model }) }}<template v-if="s.summary.cost_usd != null"> ({{ fmtUsd(s.summary.cost_usd) }})</template>
        </p>
      </template>
      <p v-else class="muted">{{ t('detail.notSummarized') }}</p>

      <h3 class="section-title">{{ t('detail.cost') }} <span class="muted small">{{ t('apiPrice') }}</span></h3>
      <table class="table">
        <thead>
          <tr><th /><th class="r">{{ t('detail.tokens') }}</th><th class="r">{{ t('detail.output') }}</th><th class="r">{{ t('detail.amount') }}</th></tr>
        </thead>
        <tbody>
          <tr v-for="r in costRows" :key="r.k">
            <td>{{ t(`detail.kind.${r.k}`) }}</td>
            <td class="r num">{{ fmtTokens(allTokens(r.tokens)) }}</td>
            <td class="r num">{{ fmtTokens(r.tokens.output) }}</td>
            <td class="r num">{{ fmtUsd(r.usd) }}</td>
          </tr>
          <tr class="totalrow">
            <td>{{ t('detail.total') }}</td>
            <td class="r num">{{ fmtTokens(allTokens(s.cost.total.tokens)) }}</td>
            <td class="r num">{{ fmtTokens(s.cost.total.tokens.output) }}</td>
            <td class="r num"><b>{{ fmtUsd(s.cost.total.usd) }}</b></td>
          </tr>
        </tbody>
      </table>
      <p class="muted small">
        {{ s.cost.models.join(', ') }}
        <template v-if="s.cost.total.unpriced.length"><br /><span class="warn">{{ t('detail.unpricedModels', { list: s.cost.total.unpriced.join(', ') }) }}</span></template>
      </p>

      <h3 class="section-title">{{ t('detail.commits', { n: s.commits.length }) }}</h3>
      <ul v-if="s.commits.length" class="commits">
        <li v-for="c in s.commits" :key="c.hash"><code>{{ c.hash }}</code> {{ c.subject }}</li>
      </ul>
      <p v-else-if="s.machine" class="muted small">{{ t('detail.remoteCommits') }}</p>
      <p v-else class="muted small">{{ t('detail.noCommits') }}</p>

      <h3 class="section-title">{{ t('detail.promptList', { n: s.prompts.length }) }}</h3>
      <ol class="prompts">
        <li v-for="(p, i) in s.prompts" :key="i">
          <span class="muted num">{{ clock(p.ts) }}</span>
          <details v-if="p.text.length > 160">
            <summary>{{ p.text.slice(0, 160) }}…</summary>
            <div class="full">{{ p.text }}</div>
          </details>
          <span v-else class="full">{{ p.text }}</span>
        </li>
      </ol>

      <footer class="muted small">
        <div class="mono">{{ s.resume_command }}</div>
        <div class="mono">{{ s.file }}</div>
      </footer>
      </template>
    </template>
  </aside>
</template>

<style scoped>
.detail{overflow-y:auto; height:100%; padding:8px 24px 32px; border-left:1px solid var(--rule); background:var(--ground);
  display:flex; flex-direction:column}
.tabs{display:flex; gap:4px; margin:16px 0 4px; border-bottom:1px solid var(--rule)}
.tabs button{border:0; background:none; padding:8px 16px; color:var(--muted); font-weight:500; border-bottom:3px solid transparent; margin-bottom:-1px}
.tabs button.on{color:var(--accent); border-bottom-color:var(--accent)}
/* 会話のときはパネルの中でスクロールさせ、入力欄を下に留める */
.conv{flex:1; min-height:360px}
.tools{display:flex; justify-content:flex-end; gap:4px}
.section-title .spacer{flex:1}
.gh{display:inline-flex; gap:10px; margin-left:4px}
.gh a{color:var(--accent); text-decoration:none; font-size:13px}
.gh a:hover{text-decoration:underline}
.icon-btn.pin{filter:grayscale(1); opacity:.55}
.icon-btn.pin.on{filter:none; opacity:1}
.btn.small{height:30px; padding:0 12px; font-size:13px}
/* 画面いっぱいのときは、読みやすい幅で真ん中に置く */
.detail.full{padding-left:max(24px, calc((100% - 960px) / 2)); padding-right:max(24px, calc((100% - 960px) / 2)); border-left:0}
/* 変更（左に一覧・右に差分）は全画面なら広く使う */
.detail.full.changes{padding-left:max(24px, calc((100% - 1600px) / 2)); padding-right:max(24px, calc((100% - 1600px) / 2))}
.head{display:grid; grid-template-columns:20px 1fr; gap:16px; align-items:start}
.square{width:16px; height:16px; border-radius:4px; margin-top:8px}
h2{font-size:22px; font-weight:400; line-height:1.35; margin:0; color:var(--ink)}
.when{font-size:14px; color:var(--ink-soft); margin-top:2px}
.meta{display:flex; flex-wrap:wrap; align-items:center; gap:6px; font-size:13px; margin-top:6px}
.actions{display:flex; gap:8px; margin:16px 0 0 36px; flex-wrap:wrap}
.resume{display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin:8px 0 0 36px; font-size:13px}
.resume a{color:var(--accent)}
.small{font-size:12px}
.bullets{margin:0 0 10px; padding-left:20px}
.bullets li{margin:4px 0}
.totalrow td{border-top:1px solid var(--rule)}
.warn{color:var(--warn)}
.commits{list-style:none; padding:0; margin:0}
.commits li{margin:4px 0}
.prompts{list-style:none; padding:0; margin:0}
.prompts li{display:grid; grid-template-columns:64px 1fr; gap:8px; padding:6px 0; border-top:1px solid var(--rule-soft)}
.prompts li:first-child{border-top:0}
.full{white-space:pre-wrap; word-break:break-word}
summary{cursor:pointer; white-space:pre-wrap; word-break:break-word}
footer{margin-top:20px; padding-top:8px; border-top:1px solid var(--rule); word-break:break-all}
</style>
