<script setup>
import { onMounted, ref, watch } from 'vue'
import { getScheduleHistory } from '../api.js'
import { fmtDuration, fmtUsd } from '../layout.js'
import { locale, t } from '../i18n.js'
import { mergeRuns } from '../plans.js'

// 予定の「すべての実行」。上に全期間の合計、下に新しい順の実行を 50 件ずつ（「さらに読む」で続き）
const props = defineProps({
  id: { type: Number, required: true },
  version: { type: Number, default: 0 }, // App が読み直すたびに増える。増えたら最初のページだけ取り直す
})
const emit = defineEmits(['open-session'])
const PAGE = 50

const runs = ref([])
const totals = ref(null)
const more = ref(false)
const error = ref('')
const loading = ref(false)
// last を渡すとその続き（同じ時刻の実行は id で続きを決めるので取りこぼさない）。
// 渡さなければ最初のページを取り直し、さらに読んだ分は後ろに残す（読み直しで消さない）
async function load(last = null) {
  loading.value = true
  try {
    const r = await getScheduleHistory(props.id, last?.started_at ?? null, last?.id ?? null)
    if (last) {
      runs.value = [...runs.value, ...r.runs]
      more.value = r.runs.length === PAGE
    } else {
      const merged = mergeRuns(r.runs, runs.value)
      // さらに読んだ分が残っていれば、その先があるかは前に読んだときのまま
      if (merged.length === r.runs.length) more.value = r.runs.length === PAGE
      runs.value = merged
    }
    totals.value = r.totals
    error.value = ''
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    loading.value = false
  }
}
onMounted(() => load())
watch(() => props.version, () => load())

const when = (ms) => new Date(ms).toLocaleString(locale(), { year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
</script>

<template>
  <div class="history">
    <p v-if="error" class="chip warn">{{ error }}</p>
    <p v-if="totals" class="totals muted small num">
      {{ t('plans.totals', totals) }}<template v-if="totals.running">{{ t('plans.totalsRunning', totals) }}</template>
      <template v-if="totals.avg_secs != null"> · {{ t('plans.avg', { d: fmtDuration(totals.avg_secs * 1000) }) }}</template>
      · <span :title="totals.unpriced ? t('unpricedHint') : t('apiPrice')">{{ t('plans.cost', { usd: `${fmtUsd(totals.usd)}${totals.unpriced ? '+' : ''}` }) }}</span>
    </p>
    <div v-for="r in runs" :key="r.id" class="run">
      <span class="muted num">{{ when(r.started_at) }}</span>
      <span class="chip" :class="r.status">{{ t(`plans.status.${r.status}`) }}</span>
      <details v-if="r.summary" class="sum">
        <summary :title="r.summary">{{ r.summary }}</summary>
      </details>
      <span v-else />
      <span class="muted small num">
        <template v-if="r.secs != null">{{ fmtDuration(r.secs * 1000) }}</template>
        <template v-if="r.usd != null"> · {{ fmtUsd(r.usd) }}{{ r.unpriced ? '+' : '' }}</template>
      </span>
      <button v-if="r.session_id" class="link" @click="emit('open-session', r.session_id)">{{ t('plans.openSession') }}</button>
      <span v-else />
    </div>
    <button v-if="more" class="btn small" :disabled="loading" @click="load(runs.at(-1))">{{ t('plans.more') }}</button>
  </div>
</template>

<style scoped>
.totals{margin:6px 0}
.run{display:grid; grid-template-columns:auto auto minmax(0, 1fr) auto auto; gap:10px; align-items:center; padding:6px 0; border-top:1px solid var(--rule-soft); font-size:13px}
.run .sum{min-width:0}
.run .sum summary{display:block; font-weight:400; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; cursor:pointer; list-style:none}
.run .sum summary::-webkit-details-marker{display:none}
.run .sum[open] summary{white-space:pre-wrap; word-break:break-word}
.chip.ok{color:var(--done)}
.chip.running{color:var(--wip)}
.chip.failed,.chip.missed{color:var(--warn)}
.chip.skipped{color:var(--talk)}
.small{font-size:12px}
.btn.small{height:28px; padding:0 10px; font-size:12px; margin-top:8px}
.link{border:0; background:none; padding:0; color:var(--accent); font-weight:500; text-decoration:underline; text-underline-offset:.2em}
</style>
