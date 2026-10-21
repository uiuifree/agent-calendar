<script setup>
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { searchSessions } from '../api.js'
import { closesOnEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'
import { DAY_MS } from '../layout.js'

// ヘッダーの検索。リポジトリ・ブランチ・作業場所・題・要約・依頼の文章から過去のセッションを探し、
// 押したら会話を開く（そこから続きの指示を送れば、そのセッションの作業場所で動く）
const emit = defineEmits(['open'])
const repoName = (repo) => repo.split('/').filter(Boolean).pop() ?? repo
const ago = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })

const input = ref(null)
const root = ref(null)
const q = ref('')
const open = ref(false)
const results = ref(null) // null = まだ探していない
const error = ref('')
// 50 件ちょうど返ってきたら、まだ先があるかもしれない（一番下に「さらに読む」を出す）
const more = ref(false)
// 絞り込み: エージェント（'' = すべて）と、最後に動いた時刻の範囲（日数。0 = すべて）
const source = ref('')
const days = ref(0)
const SOURCES = [['claude', 'source.claude'], ['codex', 'source.codex'], ['', 'search.all']]
const PERIODS = [[0, 'search.anyTime'], [7, 'search.days7'], [30, 'search.days30']]
const filter = () => ({ source: source.value, from: days.value ? Date.now() - days.value * DAY_MS : null })
// ↑↓ で選んでいる候補（-1 = 選んでいない）。結果が変わったら選び直す
const cursor = ref(-1)
watch(results, () => (cursor.value = -1))
const list = ref(null)
// 打つたびに問い合わせず、手が止まってから。遅れて届いた古い答えは捨てる
let timer
let seq = 0
watch([q, source, days], ([text]) => {
  clearTimeout(timer)
  const mine = ++seq // 打ち直したら、それまでに送った問い合わせの答えは使わない
  if (!text.trim()) {
    results.value = null
    more.value = false
    error.value = ''
    return
  }
  timer = setTimeout(async () => {
    try {
      const r = await searchSessions(text.trim(), filter())
      if (mine === seq) {
        results.value = r.sessions
        more.value = r.more
        error.value = ''
      }
    } catch (e) {
      if (mine === seq) error.value = String(e.message ?? e)
    }
  }, 250)
})
// さらに読む: 最後の 1 件より前に動いたものを足す（同じ時刻のものは id で続きを決める）。
// 読み込み中に何度押しても 1 回だけ（同じページを二重に足さないように）
const loadingMore = ref(false)
async function loadMore() {
  if (loadingMore.value) return
  loadingMore.value = true
  const mine = seq
  const last = results.value.at(-1)
  try {
    const r = await searchSessions(q.value.trim(), { ...filter(), before: last.last_ts, beforeId: last.id })
    if (mine !== seq) return
    results.value = [...results.value, ...r.sessions]
    more.value = r.more
  } catch (e) {
    if (mine === seq) error.value = String(e.message ?? e)
  } finally {
    loadingMore.value = false
  }
}

function pick(id) {
  emit('open', id)
  open.value = false
}
// ↑↓ で候補を選び、Enter で開く（日本語入力の変換中の Enter・矢印は変換のためのものなので拾わない）
async function move(step) {
  const n = results.value?.length ?? 0
  if (!n) return
  open.value = true
  // まだ選んでいなければ、↓ は先頭・↑ は最後から。端まで行ったら反対の端へ回る
  cursor.value = cursor.value < 0 ? (step > 0 ? 0 : n - 1) : (cursor.value + step + n) % n
  await nextTick()
  list.value?.querySelector('.row.on')?.scrollIntoView({ block: 'nearest' })
}
function onKey(e) {
  if (e.isComposing) return
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    move(e.key === 'ArrowDown' ? 1 : -1)
    return
  }
  if (e.key === 'Enter' && open.value && results.value?.[cursor.value]) {
    e.preventDefault()
    pick(results.value[cursor.value].id)
    return
  }
  if (!closesOnEscape(e)) return
  e.preventDefault()
  open.value = false
  input.value.blur()
}
// 外を押したら閉じる（打った言葉は残すので、戻れば同じ結果が出る）
const outside = (e) => {
  if (!root.value.contains(e.target)) open.value = false
}
onMounted(() => document.addEventListener('mousedown', outside))
onUnmounted(() => {
  document.removeEventListener('mousedown', outside)
  clearTimeout(timer)
})
// / で入ったときは、前に打った言葉を選んだ状態にする（そのまま打てば置き換わる）
defineExpose({
  focus: () => {
    input.value.focus()
    input.value.select()
  },
})
</script>

<template>
  <div ref="root" class="search">
    <input
      ref="input"
      v-model="q"
      type="search"
      spellcheck="false"
      :placeholder="t('search.placeholder')"
      :aria-label="t('search.placeholder')"
      aria-keyshortcuts="/"
      @focus="open = true"
      @input="open = true"
      @keydown="onKey"
    />
    <div v-if="open && (results || error)" ref="list" class="results">
      <div class="filters">
        <div class="segmented" role="group" :aria-label="t('search.sourceLabel')">
          <button v-for="[v, k] in SOURCES" :key="k" :class="{ on: source === v }" @click="source = v">{{ t(k) }}</button>
        </div>
        <div class="segmented" role="group" :aria-label="t('search.periodLabel')">
          <button v-for="[v, k] in PERIODS" :key="k" :class="{ on: days === v }" @click="days = v">{{ t(k) }}</button>
        </div>
      </div>
      <p v-if="error" class="chip warn">{{ error }}</p>
      <p v-else-if="!results.length" class="muted none">{{ t('search.none') }}</p>
      <button v-for="(s, i) in results ?? []" :key="s.id" class="row" :class="{ on: i === cursor }" @click="pick(s.id)" @mousemove="cursor = i">
        <span class="title">{{ s.title || t('untitled') }}</span>
        <span class="meta muted">
          {{ repoName(s.repo) }}<template v-if="s.branch"> · <code>{{ s.branch }}</code></template> ·
          <span class="num">{{ ago(s.last_ts) }}</span>
          <span v-if="s.status" class="chip" :class="s.status">{{ t(`status.${s.status}`) }}</span>
          <span v-if="s.cwd_gone" class="chip warn" :title="t('search.cwdGoneTitle')">{{ t('search.cwdGone') }}</span>
        </span>
        <span class="cwd muted" :title="s.cwd">{{ s.cwd }}</span>
      </button>
      <button v-if="!error && more && results?.length" class="more" :disabled="loadingMore" @click="loadMore">{{ t('search.more') }}</button>
    </div>
  </div>
</template>

<style scoped>
.search{position:relative; width:min(360px, 32vw)}
input{width:100%; height:40px; border:0; border-radius:20px; padding:0 16px; background:var(--panel); font:inherit; font-size:14px; color:var(--ink)}
input:focus{outline:2px solid var(--accent); background:var(--ground)}
/* 検索欄はヘッダーの右寄りにあるので、一覧は右端を揃えて左へ広げる（画面の右からはみ出さない） */
.results{position:absolute; top:44px; right:0; width:max(100%, 420px); max-width:calc(100vw - 16px); max-height:min(480px, calc(100vh - 80px)); overflow-y:auto; z-index:30;
  background:var(--ground); border-radius:12px; padding:6px; box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
.none{margin:8px 10px; font-size:13px}
.filters{display:flex; flex-wrap:wrap; gap:8px; padding:4px 6px 8px}
.segmented{display:inline-flex; border:1px solid var(--outline); border-radius:14px; overflow:hidden}
.segmented button{border:0; background:none; height:26px; padding:0 10px; font-size:12px; color:var(--ink-soft); font-weight:500}
.segmented button + button{border-left:1px solid var(--outline)}
.segmented button.on{background:var(--accent-soft); color:#041e49}
.more{display:block; width:100%; border:0; background:none; padding:8px 10px; border-radius:8px; color:var(--accent); font-weight:500; font-size:13px}
.more:hover:not(:disabled){background:var(--hover)}
.more:disabled{color:var(--muted); cursor:default}
.row{display:flex; flex-direction:column; align-items:flex-start; gap:2px; width:100%; border:0; background:none; padding:8px 10px; border-radius:8px; text-align:left; min-width:0}
.row:hover,.row.on{background:var(--hover)}
.title{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:14px; font-weight:500; color:var(--ink)}
.meta{display:flex; flex-wrap:wrap; align-items:center; gap:4px; font-size:12px; max-width:100%}
.meta code{font-size:12px}
.meta .chip{font-size:11px; padding:0 6px}
.cwd{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:11px; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace}
</style>
