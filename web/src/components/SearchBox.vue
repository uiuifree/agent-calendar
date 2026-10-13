<script setup>
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { searchSessions } from '../api.js'
import { closesOnEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'

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
// 打つたびに問い合わせず、手が止まってから。遅れて届いた古い答えは捨てる
let timer
let seq = 0
watch(q, (text) => {
  clearTimeout(timer)
  const mine = ++seq // 打ち直したら、それまでに送った問い合わせの答えは使わない
  if (!text.trim()) {
    results.value = null
    error.value = ''
    return
  }
  timer = setTimeout(async () => {
    try {
      const r = (await searchSessions(text.trim())).sessions
      if (mine === seq) {
        results.value = r
        error.value = ''
      }
    } catch (e) {
      if (mine === seq) error.value = String(e.message ?? e)
    }
  }, 250)
})

function pick(id) {
  emit('open', id)
  open.value = false
}
function onKey(e) {
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
defineExpose({ focus: () => input.value.focus() })
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
    <div v-if="open && (results || error)" class="results">
      <p v-if="error" class="chip warn">{{ error }}</p>
      <p v-else-if="!results.length" class="muted none">{{ t('search.none') }}</p>
      <button v-for="s in results ?? []" :key="s.id" class="row" @click="pick(s.id)">
        <span class="title">{{ s.title || t('untitled') }}</span>
        <span class="meta muted">
          {{ repoName(s.repo) }}<template v-if="s.branch"> · <code>{{ s.branch }}</code></template> ·
          <span class="num">{{ ago(s.last_ts) }}</span>
          <span v-if="s.status" class="chip" :class="s.status">{{ t(`status.${s.status}`) }}</span>
          <span v-if="s.cwd_gone" class="chip warn" :title="t('search.cwdGoneTitle')">{{ t('search.cwdGone') }}</span>
        </span>
        <span class="cwd muted" :title="s.cwd">{{ s.cwd }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.search{position:relative; width:min(360px, 32vw)}
input{width:100%; height:40px; border:0; border-radius:20px; padding:0 16px; background:var(--panel); font:inherit; font-size:14px; color:var(--ink)}
input:focus{outline:2px solid var(--accent); background:var(--ground)}
.results{position:absolute; top:44px; left:0; width:max(100%, 420px); max-height:min(480px, calc(100vh - 80px)); overflow-y:auto; z-index:30;
  background:var(--ground); border-radius:12px; padding:6px; box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
.none{margin:8px 10px; font-size:13px}
.row{display:flex; flex-direction:column; align-items:flex-start; gap:2px; width:100%; border:0; background:none; padding:8px 10px; border-radius:8px; text-align:left; min-width:0}
.row:hover{background:var(--hover)}
.title{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:14px; font-weight:500; color:var(--ink)}
.meta{display:flex; flex-wrap:wrap; align-items:center; gap:4px; font-size:12px; max-width:100%}
.meta code{font-size:12px}
.meta .chip{font-size:11px; padding:0 6px}
.cwd{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:11px; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace}
</style>
