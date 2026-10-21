<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { stopInstruction } from '../api.js'
import { closesOnEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'
import { askCount } from '../running.js'

// ヘッダーの「実行中 N」。押すと、実行中（許可待ちの数つき）・動いている予定・返事待ちのセッションを並べる。
// 何も動いておらず返事待ちだけなら、控えめな「返事待ち N」にする（返事待ちは直近 24 時間のものがいつもあるので目立たせない）。
// どれも無ければ出さない
const props = defineProps({
  running: { type: Object, default: null }, // /api/running の答え
})
const emit = defineEmits(['open-session', 'open-plans', 'changed'])
const repoName = (repo) => repo.split('/').filter(Boolean).pop() ?? repo
const ago = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })

const runs = computed(() => props.running?.runs ?? [])
const schedules = computed(() => props.running?.schedules ?? [])
const waiting = computed(() => props.running?.waiting ?? [])
const asks = computed(() => askCount(props.running))
const any = computed(() => runs.value.length + schedules.value.length > 0)
const onlyWaiting = computed(() => !any.value && waiting.value.length > 0)

const open = ref(false)
const root = ref(null)
const error = ref('')
const outside = (e) => {
  if (root.value && !root.value.contains(e.target)) open.value = false
}
const onKey = (e) => {
  if (open.value && closesOnEscape(e)) open.value = false
}
onMounted(() => {
  document.addEventListener('mousedown', outside)
  document.addEventListener('keydown', onKey)
})
onUnmounted(() => {
  document.removeEventListener('mousedown', outside)
  document.removeEventListener('keydown', onKey)
})

function pick(id) {
  emit('open-session', id)
  open.value = false
}
async function stop(id) {
  if (!window.confirm(t('conv.stopConfirm'))) return
  error.value = ''
  try {
    await stopInstruction(id)
    emit('changed')
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}
</script>

<template>
  <div v-if="any || onlyWaiting" ref="root" class="running">
    <button class="btn small toggle" :class="{ asks: asks > 0, quiet: onlyWaiting }" :aria-expanded="open" @click="open = !open">
      <template v-if="asks">{{ t('running.asks', { n: asks }) }}</template>
      <template v-else-if="onlyWaiting">{{ t('running.waitingButton', { n: waiting.length }) }}</template>
      <template v-else>{{ t('running.button', { n: runs.length + schedules.length }) }}</template>
    </button>
    <div v-if="open" class="menu">
      <p v-if="error" class="chip warn">{{ error }}</p>
      <template v-if="runs.length">
        <h4>{{ t('running.runs') }}</h4>
        <div v-for="r in runs" :key="r.key" class="row">
          <button class="main" :disabled="!r.session_id" @click="pick(r.session_id)">
            <span class="title">{{ r.session ? r.session.title || t('untitled') : t('running.starting') }}</span>
            <span class="meta muted">
              <template v-if="r.session">{{ repoName(r.session.repo) }}</template>
              <span v-if="r.asks" class="chip warn">{{ t('running.asksOf', { n: r.asks }) }}</span>
            </span>
          </button>
          <button v-if="r.session_id" class="btn small" @click="stop(r.session_id)">{{ t('conv.stop') }}</button>
        </div>
      </template>
      <template v-if="schedules.length">
        <h4>{{ t('running.schedules') }}</h4>
        <div v-for="p in schedules" :key="p.id" class="row">
          <button class="main" @click="emit('open-plans'); open = false">
            <span class="title">{{ p.name }}</span>
          </button>
        </div>
      </template>
      <template v-if="waiting.length">
        <h4 :title="t('running.waitingHint')">{{ t('running.waiting') }}</h4>
        <div v-for="s in waiting" :key="s.id" class="row">
          <button class="main" :title="t('running.waitingHint')" @click="pick(s.id)">
            <span class="title">{{ s.title || t('untitled') }}</span>
            <span class="meta muted">{{ repoName(s.repo) }} · <span class="num">{{ ago(s.last_ts) }}</span></span>
          </button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.running{position:relative}
.btn.small{height:30px; padding:0 12px; font-size:13px; white-space:nowrap}
.toggle{color:var(--wip); border-color:var(--wip)}
/* 許可を待っているものがあれば、塗って目立たせる */
.toggle.asks{background:var(--warn); border-color:var(--warn); color:#fff}
.toggle.asks:hover{background:#8c1d18}
/* 返事待ちだけのときは控えめに（いつもあるので目を引かない） */
.toggle.quiet{color:var(--muted); border-color:var(--rule)}
.menu{position:absolute; top:38px; right:0; width:360px; max-width:calc(100vw - 16px); max-height:min(480px, calc(100vh - 80px)); overflow-y:auto; z-index:30;
  background:var(--ground); border-radius:12px; padding:6px; box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
h4{margin:8px 10px 4px; font-size:12px; font-weight:500; color:var(--muted)}
.row{display:flex; align-items:center; gap:8px; padding-right:6px}
.main{flex:1; display:flex; flex-direction:column; align-items:flex-start; gap:2px; min-width:0; border:0; background:none; padding:6px 10px; border-radius:8px; text-align:left}
.main:hover:not(:disabled){background:var(--hover)}
.main:disabled{cursor:default}
.title{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; font-weight:500; color:var(--ink)}
.meta{display:flex; align-items:center; gap:6px; font-size:12px}
.meta .chip{font-size:11px; padding:0 6px; height:20px}
.btn.small{flex:none}
</style>
