<script setup>
import { computed } from 'vue'
import { DAY_MS, addDays, dayEntries, planPieces } from '../layout.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'

// 月表示: 6 週 × 7 日のマスに、その日のセッションとこれからの予定を時刻順に 3 件まで。残りは「他 N 件」
const props = defineProps({
  month: { type: Number, required: true }, // 表示する月の 1 日
  gridFrom: { type: Number, required: true }, // マスの左上の日（月曜）
  sessions: { type: Array, required: true },
  repos: { type: Object, required: true },
  machineLabels: { type: Object, default: null },
  selectedId: { type: String, default: null },
  now: { type: Number, required: true },
  plans: { type: Array, default: () => [] },
})
const emit = defineEmits(['select', 'open-day', 'open-plan'])

const SHOW = 3
const clock = (ms) => new Date(ms).toLocaleTimeString(locale(), { hour: 'numeric', minute: '2-digit' })
const weekdays = computed(() =>
  Array.from({ length: 7 }, (_, i) => new Date(addDays(props.gridFrom, i)).toLocaleDateString(locale(), { weekday: 'short' })),
)

const cells = computed(() => {
  const m = new Date(props.month).getMonth()
  const entries = dayEntries(props.sessions, props.gridFrom, 42)
  for (const p of planPieces(props.plans, props.gridFrom, 42)) entries[p.day].push(p)
  return entries.map((all, i) => {
    const list = all.sort((a, b) => a.start - b.start)
    const d = addDays(props.gridFrom, i)
    return {
      d,
      n: new Date(d).getDate(),
      other: new Date(d).getMonth() !== m,
      today: props.now >= d && props.now < d + DAY_MS,
      shown: list.slice(0, SHOW).map((e) => {
        if (e.plan) return e
        const info = props.repos[e.session.repo] ?? { name: e.session.repo, slot: null }
        return { ...e, color: repoColor(info.slot), name: info.name }
      }),
      more: Math.max(0, list.length - SHOW),
    }
  })
})
</script>

<template>
  <div class="month">
    <div class="head">
      <span v-for="(w, i) in weekdays" :key="i">{{ w }}</span>
    </div>
    <div class="grid">
      <div v-for="c in cells" :key="c.d" class="cell" :class="{ other: c.other }">
        <button class="date num" :class="{ today: c.today }" @click="emit('open-day', c.d)">{{ c.n }}</button>
        <template v-for="e in c.shown" :key="e.plan ? `plan-${e.plan.id}-${e.start}` : e.session.id">
        <button v-if="e.plan" class="entry" :title="`${t('plans.onCalendar')}: ${e.plan.name}`" @click="emit('open-plan', e.plan.id)">
          <i class="dot ring" :class="e.plan.status" />
          <span class="time num">{{ clock(e.start) }}</span>
          <span class="title">{{ e.plan.name }}<template v-if="e.plan.status"> · {{ t(`plans.status.${e.plan.status}`) }}</template></span>
        </button>
        <button
          v-else
          class="entry"
          :class="{ selected: e.session.id === selectedId }"
          :title="`${e.session.title || t('untitled')}\n${e.name}${machineLabels ? ` (${machineLabels[e.session.machine] ?? e.session.machine})` : ''}`"
          @click="emit('select', e.session.id)"
        >
          <i class="dot" :style="{ background: e.color }" />
          <span class="time num">{{ clock(e.start) }}</span>
          <span class="title">{{ e.session.title || t('untitled') }}</span>
        </button>
        </template>
        <button v-if="c.more" class="more" @click="emit('open-day', c.d)">{{ t('moreSessions', { n: c.more }) }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 6 週が画面の高さに収まらなければ、月の中で縦にスクロールする */
.month{display:flex; flex-direction:column; height:100%; min-height:0; overflow-y:auto}
.head{display:grid; grid-template-columns:repeat(7, 1fr); text-align:center; font-size:11px; font-weight:500; color:var(--muted);
  text-transform:uppercase; letter-spacing:.08em; padding:4px 0}
.grid{flex:1 0 auto; display:grid; grid-template-columns:repeat(7, 1fr); grid-template-rows:repeat(6, minmax(110px, 1fr));
  border-top:1px solid var(--rule); border-left:1px solid var(--rule); min-height:0}
.cell{display:flex; flex-direction:column; gap:1px; padding:4px 4px 6px; border-right:1px solid var(--rule); border-bottom:1px solid var(--rule); min-width:0; overflow:hidden}
.date{align-self:center; width:26px; height:26px; border:0; border-radius:50%; background:none; font-size:12px; font-weight:500; color:var(--ink)}
.date:hover{background:var(--hover)}
.cell.other .date{color:var(--muted)}
.date.today{background:var(--accent); color:#fff}
.entry{display:flex; align-items:center; gap:4px; width:100%; height:22px; padding:0 6px; border:0; border-radius:4px; background:none;
  font-size:12px; text-align:left; min-width:0}
.entry:hover{background:var(--hover)}
.entry.selected{background:var(--accent-faint)}
.dot{flex:none; width:8px; height:8px; border-radius:50%}
.dot.ring{box-sizing:border-box; border:2px solid var(--accent)}
.dot.ring.failed,.dot.ring.missed,.dot.ring.skipped{border-color:var(--warn)}
.time{flex:none; color:var(--ink-soft)}
.title{min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--ink)}
.more{align-self:flex-start; border:0; background:none; padding:0 6px; font-size:12px; font-weight:500; color:var(--ink-soft); border-radius:4px}
.more:hover{background:var(--hover)}
</style>
