<script setup>
import { computed, onMounted, ref } from 'vue'
import { getDirs, saveSchedule } from '../api.js'
import { useEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'
import { ALL_DAYS, WEEKDAYS } from '../plans.js'

// 予定の登録・編集。いつ動くかは 1 回だけ／曜日と時刻／時間ごと
const props = defineProps({
  plan: { type: Object, default: null }, // 編集するときの予定（id の無いもの＝コピーなら、その中身で新しく作る）。null なら新しく作る
  at: { type: Number, default: null }, // 新しく作るとき、カレンダーで押した日時（その日時・曜日を入れておく）
})
const emit = defineEmits(['close', 'saved'])
useEscape(() => emit('close'))

const pad = (n) => String(n).padStart(2, '0')
// datetime-local の入力は端末の時刻帯の「YYYY-MM-DDTHH:mm」
const toLocalInput = (ms) => {
  const d = new Date(ms)
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}
const tomorrow9 = () => {
  const d = new Date()
  d.setDate(d.getDate() + 1)
  d.setHours(9, 0, 0, 0)
  return d.getTime()
}

const p = props.plan
// カレンダーの空いた時間から開いたら、その日時の「1 回だけ」。曜日と時刻に切り替えてもその曜日・時刻が入っている
const picked = !p && props.at != null ? new Date(props.at) : null
const r = p?.repeat ?? (picked ? { kind: 'once', at: props.at, days: 1 << ((picked.getDay() + 6) % 7), minute: picked.getHours() * 60 + picked.getMinutes() } : {})
const form = ref({
  name: p?.name ?? '',
  dir: p?.dir ?? '',
  agent: p?.agent ?? 'claude',
  mode: p?.mode ?? 'read',
  worktree: p?.worktree ?? false,
  continue_session: p?.continue_session ?? false,
  prompt: p?.prompt ?? '',
  enabled: p?.enabled ?? true,
  kind: r.kind ?? 'weekly',
  at: toLocalInput(r.at ?? tomorrow9()),
  days: r.days ?? WEEKDAYS,
  time: r.minute != null ? `${pad(Math.floor(r.minute / 60))}:${pad(r.minute % 60)}` : '09:00',
  every_min: r.every_min ?? 120,
  from_hour: r.from_hour ?? 7,
  to_hour: r.to_hour ?? 22,
})

const dirs = ref([])
onMounted(async () => {
  try {
    dirs.value = (await getDirs()).dirs
  } catch {
    dirs.value = []
  }
})

const dayNames = computed(() =>
  Array.from({ length: 7 }, (_, i) => new Date(2026, 9, 5 + i).toLocaleDateString(locale(), { weekday: 'short' })),
)
const toggleDay = (i) => (form.value.days ^= 1 << i)
const EVERY = [15, 30, 60, 120, 180, 240, 360]
const HOURS = Array.from({ length: 25 }, (_, h) => h)

function repeat() {
  const f = form.value
  if (f.kind === 'once') return { kind: 'once', at: new Date(f.at).getTime() }
  const [h, m] = f.time.split(':').map(Number)
  if (f.kind === 'weekly') return { kind: 'weekly', days: f.days & ALL_DAYS, minute: h * 60 + m }
  return { kind: 'interval', days: f.days & ALL_DAYS, every_min: f.every_min, from_hour: f.from_hour, to_hour: f.to_hour }
}

const error = ref('')
const saving = ref(false)
async function save() {
  error.value = ''
  saving.value = true
  const f = form.value
  try {
    await saveSchedule(p?.id ?? null, {
      name: f.name,
      dir: f.dir,
      agent: f.agent,
      mode: f.mode,
      worktree: f.worktree,
      continue_session: f.continue_session,
      prompt: f.prompt,
      repeat: repeat(),
      enabled: f.enabled,
    })
    emit('saved')
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="dialog" role="dialog" aria-modal="true" :aria-label="p?.id != null ? t('plans.form.titleEdit') : t('plans.form.titleNew')">
      <h2>{{ p?.id != null ? t('plans.form.titleEdit') : t('plans.form.titleNew') }}</h2>
      <p v-if="error" class="chip warn err">{{ error }}</p>

      <label class="field">
        <span>{{ t('plans.form.name') }}</span>
        <input v-model="form.name" maxlength="100" />
      </label>
      <label class="field">
        <span>{{ t('plans.form.dir') }}</span>
        <input v-model="form.dir" list="aw-dirs" class="mono" placeholder="/home/…/repo" />
        <datalist id="aw-dirs"><option v-for="d in dirs" :key="d" :value="d" /></datalist>
      </label>
      <div class="cols">
        <label class="field">
          <span>{{ t('plans.form.agent') }}</span>
          <select v-model="form.agent">
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
        </label>
        <label class="field">
          <span>{{ t('plans.form.mode') }}</span>
          <select v-model="form.mode">
            <option value="read">{{ t('conv.modeRead') }}</option>
            <option value="edit">{{ t('conv.modeEdit') }}</option>
            <option value="auto">{{ t('conv.modeAuto') }}</option>
          </select>
        </label>
      </div>
      <label class="field">
        <span>{{ t('plans.form.prompt') }}</span>
        <textarea v-model="form.prompt" rows="4" />
      </label>
      <label class="check"><input v-model="form.worktree" type="checkbox" /> {{ t('plans.form.worktree') }}</label>
      <div class="field">
        <span>{{ t('plans.form.session') }}</span>
        <label class="check"><input v-model="form.continue_session" type="radio" :value="false" /> {{ t('plans.form.newSession') }}</label>
        <label class="check"><input v-model="form.continue_session" type="radio" :value="true" /> {{ t('plans.form.continueSession') }}</label>
      </div>

      <div class="field">
        <span>{{ t('plans.form.repeat') }}</span>
        <div class="segmented">
          <button v-for="k in ['once', 'weekly', 'interval']" :key="k" :class="{ on: form.kind === k }" @click="form.kind = k">
            {{ t(`plans.form.${k}`) }}
          </button>
        </div>
      </div>
      <label v-if="form.kind === 'once'" class="field">
        <span>{{ t('plans.form.at') }}</span>
        <input v-model="form.at" type="datetime-local" />
      </label>
      <template v-else>
        <div class="field">
          <span>{{ t('plans.form.days') }}</span>
          <div class="days">
            <button v-for="(n, i) in dayNames" :key="i" :class="{ on: form.days & (1 << i) }" @click="toggleDay(i)">{{ n }}</button>
          </div>
        </div>
        <label v-if="form.kind === 'weekly'" class="field">
          <span>{{ t('plans.form.time') }}</span>
          <input v-model="form.time" type="time" />
        </label>
        <div v-else class="cols">
          <label class="field">
            <span>{{ t('plans.form.every') }}</span>
            <select v-model.number="form.every_min">
              <option v-for="m in EVERY" :key="m" :value="m">
                {{ m % 60 === 0 ? t('plans.everyHours', { n: m / 60 }) : t('plans.everyMinutes', { n: m }) }}
              </option>
            </select>
          </label>
          <div class="field">
            <span>{{ t('plans.form.between') }}</span>
            <span class="row">
              <select v-model.number="form.from_hour"><option v-for="h in HOURS.slice(0, 24)" :key="h" :value="h">{{ h }}:00</option></select>
              –
              <select v-model.number="form.to_hour"><option v-for="h in HOURS.slice(1)" :key="h" :value="h">{{ h }}:00</option></select>
            </span>
          </div>
        </div>
      </template>
      <label class="check"><input v-model="form.enabled" type="checkbox" /> {{ t('plans.form.enabled') }}</label>
      <p class="muted small">{{ t('plans.form.missedNote') }}</p>

      <div class="actions">
        <button class="btn" @click="emit('close')">{{ t('plans.form.cancel') }}</button>
        <button class="btn primary" :disabled="saving" @click="save">{{ t('plans.form.save') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.backdrop{position:fixed; inset:0; background:rgba(32,33,36,.4); display:grid; place-items:center; z-index:20}
.dialog{width:min(560px, calc(100vw - 32px)); max-height:calc(100vh - 48px); overflow-y:auto; background:var(--ground); border-radius:28px; padding:24px;
  box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
h2{font-size:22px; font-weight:400; margin:0 0 16px}
.err{display:block; height:auto; white-space:normal; border-radius:8px; padding:6px 10px; margin-bottom:12px}
.field{display:flex; flex-direction:column; gap:6px; margin-bottom:14px}
.field > span:first-child{font-weight:500; font-size:13px}
.cols{display:grid; grid-template-columns:1fr 1fr; gap:12px}
input,select,textarea{font:inherit; border:1px solid var(--outline); border-radius:8px; padding:6px 10px; background:var(--ground)}
input:not([type="checkbox"]):not([type="radio"]),select{height:36px}
textarea{resize:vertical}
.check{display:flex; align-items:center; gap:8px; font-size:14px; margin-bottom:8px}
.check input{width:18px; height:18px; accent-color:var(--accent)}
.segmented{display:inline-flex; align-self:flex-start; border:1px solid var(--outline); border-radius:18px; overflow:hidden}
.segmented button{border:0; background:none; height:32px; padding:0 14px; color:var(--ink-soft); font-weight:500}
.segmented button + button{border-left:1px solid var(--outline)}
.segmented button.on{background:var(--accent-soft); color:#041e49}
.days{display:flex; gap:6px; flex-wrap:wrap}
.days button{width:40px; height:32px; border:1px solid var(--outline); border-radius:16px; background:var(--ground); color:var(--ink-soft)}
.days button.on{background:var(--accent); border-color:var(--accent); color:#fff}
.row{display:flex; align-items:center; gap:6px}
.small{font-size:12px}
.actions{display:flex; justify-content:flex-end; gap:8px; margin-top:8px}
</style>
