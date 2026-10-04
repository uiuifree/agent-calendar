<script setup>
import { onMounted, ref } from 'vue'
import { checkUpdate, getSettings, getUpdate, saveSettings } from '../api.js'
import { useEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'

// 自動で動かす時間帯と間隔。保存すると裏の処理は次の 1 分から新しい設定で動く
const emit = defineEmits(['close', 'saved'])
useEscape(() => emit('close'))

const form = ref(null)
// 組織は空白区切り、フォルダは 1 行に 1 つで入力し、保存するときに配列へ戻す
const owners = ref('')
const roots = ref('')
const words = (s) => s.split(/[\s,]+/).filter(Boolean)
const lines = (s) => s.split('\n').map((l) => l.trim()).filter(Boolean)
const summarizing = ref(true)
const error = ref('')
const saved = ref(false)

const HOURS = Array.from({ length: 25 }, (_, h) => h)
const REFRESH = [5, 10, 15, 30, 60]
const SUMMARY = [15, 30, 60, 120, 180, 360]
const hourLabel = (h) => `${h}:00`
const span = (m) => (m < 60 ? t('settings.minutes', { n: m }) : t('settings.hourUnit', { n: m / 60 }))

// 版: いまの版と、確かめた最新の版
const version = ref(null)
const checking = ref(false)
async function checkNow() {
  checking.value = true
  try {
    version.value = await checkUpdate()
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    checking.value = false
  }
}

onMounted(async () => {
  getUpdate()
    .then((v) => (version.value = v))
    .catch(() => {})
  try {
    const r = await getSettings()
    form.value = { ...r.settings }
    owners.value = (r.settings.github_owners ?? []).join(' ')
    roots.value = (r.settings.repo_roots ?? []).join('\n')
    summarizing.value = r.summarizing
  } catch (e) {
    error.value = String(e)
  }
})

async function save() {
  error.value = ''
  try {
    await saveSettings({ ...form.value, github_owners: words(owners.value), repo_roots: lines(roots.value) })
    saved.value = true
    emit('saved')
    setTimeout(() => emit('close'), 600)
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="dialog" role="dialog" aria-modal="true" :aria-label="t('settings.title')" :lang="locale()">
      <h2>{{ t('settings.title') }}</h2>
      <p v-if="error" class="chip warn">{{ error }}</p>
      <template v-if="form">
        <label class="field">
          <span>{{ t('settings.window') }}</span>
          <span class="row">
            <select v-model.number="form.start_hour">
              <option v-for="h in HOURS.slice(0, 24)" :key="h" :value="h">{{ hourLabel(h) }}</option>
            </select>
            {{ t('settings.to') }}
            <select v-model.number="form.end_hour">
              <option v-for="h in HOURS" :key="h" :value="h">{{ hourLabel(h) }}</option>
            </select>
          </span>
          <small class="muted">{{ t('settings.allDay') }}</small>
        </label>
        <label class="field">
          <span>{{ t('settings.refresh') }}</span>
          <select v-model.number="form.refresh_minutes">
            <option v-for="m in REFRESH" :key="m" :value="m">{{ span(m) }}</option>
          </select>
        </label>
        <label class="field">
          <span>{{ t('settings.summary') }}</span>
          <select v-model.number="form.summary_minutes" :disabled="!summarizing">
            <option v-for="m in SUMMARY" :key="m" :value="m">{{ span(m) }}</option>
          </select>
          <small v-if="!summarizing" class="muted">{{ t('settings.summaryOff') }}</small>
        </label>
        <label class="field">
          <span>{{ t('settings.owners') }}</span>
          <input v-model="owners" type="text" spellcheck="false" placeholder="my-org my-user" />
          <small class="muted">{{ t('settings.ownersHint') }}</small>
        </label>
        <label class="field">
          <span>{{ t('settings.roots') }}</span>
          <textarea v-model="roots" rows="3" spellcheck="false" placeholder="/home/me/projects" />
          <small class="muted">{{ t('settings.rootsHint') }}</small>
        </label>
        <div class="field">
          <span>{{ t('settings.updates') }}</span>
          <label class="check"><input v-model="form.update_check" type="checkbox" /> {{ t('settings.updateCheck') }}</label>
          <label class="check"><input v-model="form.auto_update" type="checkbox" /> {{ t('settings.autoUpdate') }}</label>
          <span class="row small muted">
            <span v-if="version">
              {{ t('settings.version', { current: version.current, latest: version.latest?.version ?? '—' }) }}
              <template v-if="version.error"> · {{ /no release/.test(version.error) ? t('settings.noRelease') : version.error }}</template>
            </span>
            <button class="btn tiny" type="button" :disabled="checking" @click="checkNow">{{ t('settings.checkNow') }}</button>
          </span>
        </div>
        <div class="actions">
          <button class="btn" @click="emit('close')">{{ t('settings.cancel') }}</button>
          <button class="btn primary" @click="save">{{ saved ? t('settings.saved') : t('settings.save') }}</button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.backdrop{position:fixed; inset:0; background:rgba(32,33,36,.4); display:grid; place-items:center; z-index:20}
.dialog{width:min(520px, calc(100vw - 32px)); max-height:calc(100vh - 48px); overflow-y:auto; background:var(--ground); border-radius:28px; padding:24px;
  box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
h2{font-size:22px; font-weight:400; margin:0 0 16px}
.field{display:flex; flex-direction:column; gap:6px; margin-bottom:16px}
.field > span:first-child{font-weight:500; font-size:14px}
.row{display:flex; align-items:center; gap:8px}
select{height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 8px; background:var(--ground); min-width:96px}
input,textarea{border:1px solid var(--outline); border-radius:8px; padding:8px 10px; font:inherit; background:var(--ground)}
textarea{resize:vertical; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; font-size:13px}
small{font-size:12px; line-height:1.5}
.check{display:flex; align-items:center; gap:8px; font-size:14px}
.small{font-size:12px}
.btn.tiny{height:26px; padding:0 10px; font-size:12px}
.actions{display:flex; justify-content:flex-end; gap:8px; margin-top:8px}
</style>
