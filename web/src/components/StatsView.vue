<script setup>
import { computed, ref, watch } from 'vue'
import { getStats, setProject } from '../api.js'
import { addDays, fmtHours, fmtUsd } from '../layout.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'

// 案件ごとの作業時間と API 単価換算の金額。案件はリポジトリごとにここで選ぶ（推測では埋めない）
const props = defineProps({
  from: { type: Number, required: true }, // 表示中の週の月曜
  version: { type: Number, required: true }, // 読み直すたびに増える。増えたら取り直す
  hosts: { type: Array, default: null }, // 選んだホストの id。null = すべて
})

const mode = ref('week')
const range = computed(() => {
  if (mode.value === 'week') return [props.from, addDays(props.from, 7)]
  const d = new Date(props.from)
  return [new Date(d.getFullYear(), d.getMonth(), 1).getTime(), new Date(d.getFullYear(), d.getMonth() + 1, 1).getTime()]
})
const label = computed(() => {
  const [a, b] = range.value.map((x) => new Date(x))
  if (mode.value === 'month') return a.toLocaleDateString(locale(), { year: 'numeric', month: 'long' })
  return new Intl.DateTimeFormat(locale(), { month: 'short', day: 'numeric' }).formatRange(a, new Date(b.getTime() - 1))
})

const data = ref(null)
const error = ref('')
async function load() {
  error.value = ''
  try {
    data.value = await getStats(...range.value, props.hosts)
  } catch (e) {
    error.value = String(e)
  }
}
watch([range, () => props.version, () => props.hosts], load, { immediate: true, deep: true })

// 候補はこれまでに割り当てた案件
const suggestions = computed(() => data.value?.known_projects ?? [])

async function assign(repo, ev) {
  try {
    await setProject(repo, ev.target.value)
    await load()
  } catch (e) {
    error.value = String(e)
  }
}
</script>

<template>
  <div class="stats">
    <div class="bar">
      <div class="segmented">
        <button :class="{ on: mode === 'week' }" @click="mode = 'week'">{{ t('stats.week') }}</button>
        <button :class="{ on: mode === 'month' }" @click="mode = 'month'">{{ t('stats.month') }}</button>
      </div>
      <span class="label num">{{ label }}</span>
      <span v-if="error" class="chip warn">{{ error }}</span>
    </div>

    <template v-if="data">
      <div class="cards">
        <div class="card">
          <span class="muted">{{ t('stats.activeTime') }}</span>
          <b class="num">{{ fmtHours(data.total.secs) }}</b>
        </div>
        <div class="card">
          <span class="muted">{{ t('apiPrice') }}</span>
          <b class="num">{{ fmtUsd(data.total.usd) }}</b>
        </div>
      </div>

      <table class="table">
        <thead>
          <tr>
            <th>{{ t('stats.projectRepo') }}</th>
            <th class="r">{{ t('stats.activeTime') }}</th>
            <th class="r">{{ t('stats.sessions') }}</th>
            <th class="r">{{ t('apiPrice') }}</th>
            <th>{{ t('stats.assign') }}</th>
          </tr>
        </thead>
        <tbody v-for="p in data.projects" :key="p.project">
          <tr class="project">
            <td>{{ p.project || t('stats.unassigned') }}</td>
            <td class="r num">{{ fmtHours(p.secs) }}</td>
            <td class="r num">{{ p.sessions }}</td>
            <td class="r num">{{ fmtUsd(p.usd) }}</td>
            <td />
          </tr>
          <tr v-for="r in p.repos" :key="r.repo" class="repo">
            <td :title="r.repo"><i class="dot" :style="{ background: repoColor(r.slot) }" />{{ r.name }}</td>
            <td class="r num">{{ fmtHours(r.secs) }}</td>
            <td class="r num">{{ r.sessions }}</td>
            <td class="r num" :title="r.unpriced.length ? r.unpriced.join(', ') : ''">{{ fmtUsd(r.usd) }}{{ r.unpriced.length ? '+' : '' }}</td>
            <td>
              <input
                class="assign"
                list="aw-projects"
                :value="p.project"
                :placeholder="t('stats.unassigned')"
                :aria-label="t('stats.assignLabel', { repo: r.name })"
                @change="assign(r.repo, $event)"
              />
            </td>
          </tr>
        </tbody>
      </table>
      <datalist id="aw-projects">
        <option v-for="s in suggestions" :key="s" :value="s" />
      </datalist>

      <p class="muted foot">
        {{ t('stats.noteTime', { gap: data.gap_secs / 60 }) }}<br />
        {{ t('stats.noteCost') }}
      </p>
    </template>
  </div>
</template>

<style scoped>
.stats{max-width:960px; padding-top:8px; container-type:inline-size}
.bar{display:flex; align-items:center; gap:12px; margin-bottom:16px}
.label{font-size:16px; color:var(--ink)}
.segmented{display:inline-flex; border:1px solid var(--outline); border-radius:18px; overflow:hidden}
.segmented button{border:0; background:none; height:32px; padding:0 16px; color:var(--ink-soft); font-weight:500}
.segmented button + button{border-left:1px solid var(--outline)}
.segmented button.on{background:var(--accent-soft); color:#041e49}
.cards{display:flex; gap:16px; margin-bottom:20px}
.card{display:flex; flex-direction:column; gap:2px; min-width:200px; padding:14px 18px; border:1px solid var(--rule); border-radius:12px}
.card .muted{font-size:13px}
.card b{font-size:28px; font-weight:400; color:var(--ink)}
.project td{font-weight:500; padding-top:16px}
.repo td:first-child{padding-left:28px; color:var(--ink-soft)}
.dot{display:inline-block; width:10px; height:10px; border-radius:50%; margin-right:8px}
.assign{width:10em; height:30px; border:1px solid var(--rule); border-radius:6px; background:var(--ground); padding:0 8px}
.assign:focus{border-color:var(--accent); outline:none}
.foot{font-size:12px; margin-top:20px; line-height:1.7}
/* 数字は折り返さない（「40.3 時間」が 2 行に割れないように） */
.r{white-space:nowrap}
.repo td:first-child{white-space:nowrap}
/* 右にパネルが出るなどして狭いときは、セッション数の列を省き、割り当ての欄を縮める（右にはみ出さない） */
@container (max-width: 680px){
  .table th:nth-child(3), .table td:nth-child(3){display:none}
  .assign{width:7.5em}
  .cards .card{min-width:0; flex:1}
}
</style>
