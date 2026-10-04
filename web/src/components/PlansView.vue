<script setup>
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { deleteSchedule, getSchedules, runSchedule } from '../api.js'
import { locale, t } from '../i18n.js'
import { copyOf, describe } from '../plans.js'
import PlanForm from './PlanForm.vue'

// 予定の一覧。最近の実行の結果から、そのセッション（会話）を開ける
const props = defineProps({
  version: { type: Number, required: true }, // 読み直すたびに増える
})
const emit = defineEmits(['open-session', 'changed']) // changed: 予定が変わった（カレンダーを読み直す）

const plans = ref([])
const error = ref('')
const notice = ref('')
const editing = ref(undefined) // undefined = 閉じている、null = 新規、予定 = 編集

async function load() {
  try {
    plans.value = (await getSchedules()).schedules
    error.value = ''
  } catch (e) {
    error.value = String(e)
  }
}
watch(() => props.version, load, { immediate: true })
// 実行中の予定があるあいだは、終わったことが分かるように見に行く
let timer
onMounted(() => {
  timer = setInterval(() => {
    if (plans.value.some((p) => p.runs.some((r) => r.status === 'running'))) load()
  }, 5000)
})
onUnmounted(() => clearInterval(timer))

const when = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', weekday: 'short', hour: 'numeric', minute: '2-digit' })
const shortDir = (d) => d.split('/').filter(Boolean).slice(-2).join('/')

async function runNow(p) {
  try {
    await runSchedule(p.id)
    notice.value = t('plans.started')
    await load()
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}

async function remove(p) {
  if (!window.confirm(t('plans.removeConfirm', { name: p.name }))) return
  await deleteSchedule(p.id)
  emit('changed')
}

async function saved() {
  editing.value = undefined
  emit('changed')
}
</script>

<template>
  <div class="plans">
    <div class="bar">
      <button class="btn primary" @click="editing = null">＋ {{ t('plans.add') }}</button>
      <span v-if="notice" class="muted">{{ notice }}</span>
      <span v-if="error" class="chip warn">{{ error }}</span>
    </div>
    <p v-if="!plans.length && !error" class="muted">{{ t('plans.empty') }}</p>

    <article v-for="p in plans" :key="p.id" class="card" :class="{ off: !p.enabled }">
      <header>
        <div class="title">
          <h3>{{ p.name }}</h3>
          <div class="when">
            <b>{{ describe(p.repeat) }}</b>
            <span class="muted">· {{ !p.enabled ? t('plans.off') : p.next_at ? t('plans.next', { when: when(p.next_at) }) : t('plans.noNext') }}</span>
          </div>
        </div>
        <div class="actions">
          <button class="btn" @click="runNow(p)">{{ t('plans.runNow') }}</button>
          <button class="btn" @click="editing = p">{{ t('plans.edit') }}</button>
          <button class="btn" @click="editing = copyOf(p)">{{ t('plans.copy') }}</button>
          <button class="btn" @click="remove(p)">{{ t('plans.remove') }}</button>
        </div>
      </header>
      <div class="meta">
        <code :title="p.dir">{{ shortDir(p.dir) }}</code>
        <span class="chip src">{{ t(`source.${p.agent}`) }}</span>
        <span class="chip src">{{ t({ read: 'conv.modeRead', edit: 'conv.modeEdit', auto: 'conv.modeAuto' }[p.mode] ?? 'conv.modeRead') }}</span>
        <span v-if="p.worktree" class="chip src">worktree</span>
        <span class="chip src">{{ p.continue_session ? t('plans.form.continueSession') : t('plans.form.newSession') }}</span>
      </div>
      <p class="prompt">{{ p.prompt }}</p>

      <details class="runs" :open="p.runs.length > 0 && p.runs[0].status === 'running'">
        <summary>{{ t('plans.runs') }}<span v-if="p.runs[0]" class="chip" :class="p.runs[0].status">{{ t(`plans.status.${p.runs[0].status}`) }}</span></summary>
        <p v-if="!p.runs.length" class="muted small">{{ t('plans.noRuns') }}</p>
        <div v-for="r in p.runs" :key="r.id" class="run">
          <span class="muted num">{{ when(r.started_at) }}</span>
          <span class="chip" :class="r.status">{{ t(`plans.status.${r.status}`) }}</span>
          <span class="sum">{{ r.summary }}</span>
          <span v-if="r.branch && r.changes" class="muted small">{{ t('plans.changes', { n: r.changes, branch: r.branch }) }}</span>
          <button v-if="r.session_id" class="link" @click="emit('open-session', r.session_id)">{{ t('plans.openSession') }}</button>
        </div>
      </details>
    </article>

    <PlanForm v-if="editing !== undefined" :plan="editing" @close="editing = undefined" @saved="saved" />
  </div>
</template>

<style scoped>
.plans{max-width:960px; padding-top:8px}
.bar{display:flex; align-items:center; gap:12px; margin-bottom:16px}
.card{border:1px solid var(--rule); border-radius:12px; padding:14px 18px; margin-bottom:12px}
.card.off{opacity:.6}
header{display:flex; justify-content:space-between; gap:12px; align-items:flex-start; flex-wrap:wrap}
h3{font-size:16px; font-weight:500; margin:0}
.when{font-size:13px; margin-top:2px}
.when b{font-weight:500; color:var(--accent)}
.actions{display:flex; gap:6px}
.actions .btn{height:32px; padding:0 12px}
.meta{display:flex; flex-wrap:wrap; gap:6px; align-items:center; margin:8px 0 4px; font-size:12px}
.prompt{margin:6px 0; white-space:pre-wrap; color:var(--ink-soft); font-size:13px; max-height:4.8em; overflow:hidden}
.runs summary{cursor:pointer; font-size:13px; font-weight:500; display:flex; align-items:center; gap:8px}
.run{display:grid; grid-template-columns:auto auto minmax(0, 1fr) auto auto; gap:10px; align-items:center; padding:6px 0; border-top:1px solid var(--rule-soft); font-size:13px}
.run .sum{min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.chip.ok{color:var(--done)}
.chip.running{color:var(--wip)}
.chip.failed,.chip.missed{color:var(--warn)}
.chip.skipped{color:var(--talk)}
.small{font-size:12px}
.link{border:0; background:none; padding:0; color:var(--accent); font-weight:500; text-decoration:underline; text-underline-offset:.2em}
</style>
