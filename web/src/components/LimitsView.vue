<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { getLimits } from '../api.js'
import { t } from '../i18n.js'
import { asOfLabel, clockAt, resetLabel, statuslineSetup, usageLevel, windowName } from '../limits.js'

// 利用枠の画面: エージェントごとに、5 時間・7 日・モデルごとの枠の使い具合と戻る時刻を並べる。
// 値は最後に記録されたときのもの（statusline が残した分・記録の中の上限の知らせ）なので、いつの時点かも出す
const data = ref(null)
const error = ref('')
// 戻るまでの時間を進めるための今の時刻
const now = ref(Date.now())

async function load() {
  try {
    data.value = await getLimits()
    error.value = ''
  } catch (e) {
    error.value = String(e.message ?? e)
  }
  now.value = Date.now()
}
// 開いているあいだは 1 分ごとに取り直す
let timer
onMounted(() => {
  load()
  timer = setInterval(load, 60 * 1000)
})
onUnmounted(() => clearInterval(timer))

const copied = ref(null)
let copiedTimer
async function copy(agent, text) {
  try {
    await navigator.clipboard.writeText(text)
    copied.value = agent
    clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => (copied.value = null), 2000)
  } catch (e) {
    error.value = t('detail.copyFailed', { e })
  }
}
onUnmounted(() => clearTimeout(copiedTimer))
</script>

<template>
  <div class="limits">
    <div class="bar">
      <span class="muted small">{{ t('limits.refreshNote') }}</span>
      <span class="spacer" />
      <span v-if="error" class="chip warn">{{ error }}</span>
      <button class="btn small" @click="load">{{ t('reload') }}</button>
    </div>
    <div v-if="data" class="cards">
      <article v-for="a in data.agents" :key="a.agent" class="card">
        <header>
          <h3>{{ t(`limits.agent.${a.agent}`) }}</h3>
          <span v-if="a.plan" class="chip src">{{ t('limits.plan', { plan: a.plan }) }}</span>
          <span class="spacer" />
          <span v-if="a.observed_at && a.windows.length" class="muted small num" :title="clockAt(a.observed_at)">{{ asOfLabel(a.observed_at, now) }}</span>
        </header>

        <div v-for="(w, i) in a.windows" :key="i" class="window">
          <div class="line">
            <span class="name">{{ windowName(w) }}</span>
            <span v-if="w.exhausted" class="chip warn">{{ t('limits.exhausted') }}</span>
            <span class="spacer" />
            <b class="num pct">{{ w.used_percent == null ? '—' : `${Math.round(w.used_percent)}%` }}</b>
          </div>
          <div class="meter" :class="usageLevel(w.used_percent)" aria-hidden="true">
            <i :style="{ width: `${Math.min(100, w.used_percent ?? (w.exhausted ? 100 : 0))}%` }" />
          </div>
          <div v-if="w.resets_at" class="muted small num">{{ resetLabel(w.resets_at, now) }}</div>
        </div>

        <template v-if="!a.windows.length">
          <!-- statusline に登録すると値が取れる（Claude Code と Gemini） -->
          <div v-if="a.setup === 'statusline' && statuslineSetup(a.agent)" class="setup">
            <p>{{ t('limits.setup', { file: statuslineSetup(a.agent).file }) }}</p>
            <div class="snippet">
              <code>{{ statuslineSetup(a.agent).snippet }}</code>
              <button class="btn small" @click="copy(a.agent, statuslineSetup(a.agent).snippet)">
                {{ copied === a.agent ? t('conv.copied') : t('conv.copy') }}
              </button>
            </div>
            <p class="muted small">{{ t('limits.setupReplace') }}</p>
          </div>
          <p v-if="a.reset" class="muted">
            {{ a.observed_at ? t('limits.resetSince', { when: clockAt(a.observed_at) }) : t('limits.resetSinceUnknown') }}
          </p>
          <p v-if="!a.setup && !a.reset" class="muted">{{ t('limits.noData') }}</p>
          <p v-if="!a.setup && (a.agent === 'codex' || a.agent === 'muse')" class="muted small">
            {{ t(a.agent === 'codex' ? 'limits.noteCodex' : 'limits.noteMuse') }}
          </p>
        </template>
      </article>
    </div>
  </div>
</template>

<style scoped>
.limits{max-width:960px; padding-top:8px}
.bar{display:flex; align-items:center; gap:12px; margin-bottom:16px}
.spacer{flex:1}
.small{font-size:12px}
.btn.small{height:28px; padding:0 10px; font-size:12px; white-space:nowrap}
.cards{display:grid; grid-template-columns:repeat(auto-fill, minmax(320px, 1fr)); gap:16px}
.card{border:1px solid var(--rule); border-radius:12px; padding:14px 18px}
.card header{display:flex; align-items:center; gap:8px; margin-bottom:8px}
h3{font-size:16px; font-weight:500; margin:0}
.window{padding:8px 0; border-top:1px solid var(--rule-soft)}
.window:first-of-type{border-top:0}
.line{display:flex; align-items:center; gap:8px}
.name{font-size:14px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.pct{font-weight:500}
/* 使用率のバー。70% から橙、90% から赤 */
.meter{height:8px; border-radius:4px; background:var(--rule-soft); overflow:hidden; margin:6px 0 4px}
.meter i{display:block; height:100%; background:var(--accent)}
.meter.mid i{background:var(--talk)}
.meter.high i{background:var(--warn)}
.setup p{margin:4px 0}
.snippet{display:flex; align-items:flex-start; gap:8px; margin:6px 0}
.snippet code{flex:1; min-width:0; padding:6px 8px; white-space:pre-wrap; word-break:break-all}
</style>
