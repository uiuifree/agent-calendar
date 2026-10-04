<script setup>
import { ref } from 'vue'
import { answerPermission } from '../api.js'
import { t } from '../i18n.js'

// 実行中の Claude からの許可の問い合わせ（{ request_id, tool, text }）。押した答えを返し、結果をその場に残す
const props = defineProps({
  ask: { type: Object, required: true },
})
const answered = ref(null) // 'allowed' | 'remembered' | 'denied'
const error = ref('')
const busy = ref(false)

async function answer(allow, remember = false) {
  busy.value = true
  error.value = ''
  try {
    await answerPermission(props.ask.request_id, allow, remember)
    answered.value = !allow ? 'denied' : remember ? 'remembered' : 'allowed'
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="ask" :class="answered">
    <div class="q">{{ t('conv.askTitle', { tool: ask.tool }) }}</div>
    <pre v-if="ask.text" class="cmd">{{ ask.text }}</pre>
    <div class="row">
      <template v-if="!answered">
        <button class="btn small allow" :disabled="busy" @click="answer(true)">{{ t('conv.allow') }}</button>
        <button v-if="ask.rule" class="btn small allow" :disabled="busy" :title="t('conv.rememberHint')" @click="answer(true, true)">
          {{ t('conv.remember') }} <code>{{ ask.rule }}</code>
        </button>
        <button class="btn small" :disabled="busy" @click="answer(false)">{{ t('conv.deny') }}</button>
      </template>
      <span v-else class="result">{{ t(`conv.${answered}`, { rule: ask.rule }) }}</span>
      <span v-if="error" class="chip warn">{{ error }}</span>
    </div>
  </div>
</template>

<style scoped>
.ask{margin:8px 0; padding:10px 12px; border:1px solid var(--talk); border-radius:12px; background:var(--ground)}
.ask.allowed{border-color:var(--rule)}
.ask.denied{border-color:var(--rule)}
.q{font-size:13px; font-weight:500; color:var(--talk)}
.ask.allowed .q,.ask.denied .q{color:var(--ink-soft)}
.cmd{margin:6px 0; padding:6px 8px; background:var(--panel); border-radius:6px; font-size:12px; white-space:pre-wrap; word-break:break-word; max-height:160px; overflow:auto}
.row{display:flex; align-items:center; gap:8px; flex-wrap:wrap}
.btn code{font-size:12px; background:none}
.btn.small{height:30px; padding:0 14px; font-size:13px}
.btn.allow{color:var(--accent); border-color:var(--accent)}
.result{font-size:12px; color:var(--muted)}
</style>
