<script setup>
import { computed, onMounted, ref } from 'vue'
import { DAY_MS, addDays, assignLanes, planPieces, segments, splitByDay } from '../layout.js'
import { repoColor, textOn } from '../colors.js'
import { locale, t } from '../i18n.js'
import { everyLabel } from '../plans.js'

// 時間軸の表示（日は 1 列、週は 7 列）。縦に 0〜24 時、横に日を並べ、セッションを塗った帯として置く。
// これから動く予定は塗らずに枠だけで置く（まだ起きていないことが見て分かるように）。
// 実行中・失敗など、まだ正常に終わっていない回も状態を添えて残す。
// これから動く回はセッションと幅を分け合わず、列の幅いっぱいに上へ重ねる（今動いているセッションと重なっても潰れないように）
const props = defineProps({
  from: { type: Number, required: true },
  days: { type: Number, default: 7 },
  sessions: { type: Array, required: true },
  repos: { type: Object, required: true },
  selectedId: { type: String, default: null },
  now: { type: Number, required: true },
  machineLabels: { type: Object, default: () => ({}) },
  plans: { type: Array, default: () => [] },
})
const emit = defineEmits(['select', 'create-at', 'open-plan'])

const HOUR_PX = 48
const px = (ms) => (ms / 3600000) * HOUR_PX

const days = computed(() =>
  Array.from({ length: props.days }, (_, i) => {
    const start = addDays(props.from, i)
    const d = new Date(start)
    return {
      start,
      weekday: d.toLocaleDateString(locale(), { weekday: 'short' }),
      date: d.getDate(),
      today: props.now >= start && props.now < start + DAY_MS,
    }
  }),
)
const hourLabel = (h) => new Date(2000, 0, 1, h).toLocaleTimeString(locale(), { hour: 'numeric' })

const pieces = computed(() => {
  const out = []
  for (const s of props.sessions) {
    const info = props.repos[s.repo] ?? { name: s.repo, slot: null }
    const color = repoColor(info.slot)
    for (const seg of segments(s.buckets)) {
      for (const p of splitByDay(seg, props.from, props.days)) out.push({ ...p, session: s, name: info.name, color, text: textOn(color) })
    }
  }
  // 過ぎた回（状態つき）はセッションと横に並べ、これから動く回だけ幅いっぱいに重ねる
  const plans = planPieces(props.plans, props.from, props.days)
  return [...assignLanes([...out, ...plans.filter((p) => p.plan.status)]), ...assignLanes(plans.filter((p) => !p.plan.status))]
})
const byDay = computed(() => days.value.map((_, i) => pieces.value.filter((p) => p.day === i)))

const clock = (ms) => new Date(ms).toLocaleTimeString(locale(), { hour: 'numeric', minute: '2-digit' })

function pieceStyle(p) {
  return {
    top: `${px(p.start - p.dayStart)}px`,
    height: `${Math.max(px(p.end - p.start) - 1, 6)}px`,
    left: `calc(${(p.lane / p.lanes) * 100}% + 2px)`,
    width: `calc(${100 / p.lanes}% - 4px)`,
    ...(p.plan ? {} : { background: p.color, color: p.text }),
  }
}
const planWhen = (p) => {
  if (p.plan.status) return `${clock(p.start)} · ${t(`plans.status.${p.plan.status}`)}`
  return p.plan.every_min ? `${clock(p.start)} – ${clock(p.end)} · ${everyLabel(p.plan.every_min)}` : clock(p.start)
}
const agentName = (a) => t(`source.${a}`)
// 帯の高さで出す中身を変える（低い帯は題だけ、高い帯は題・リポジトリ・時刻）
const roomy = (p) => px(p.end - p.start) >= 44
const visible = (p) => px(p.end - p.start) >= 18

// 空いた時間を押したら、その日時（15 分単位）で予定を作る。セッションの帯を押したときは何もしない
function onSlot(e, dayStart) {
  if (e.target.closest('.event')) return
  const y = e.clientY - e.currentTarget.getBoundingClientRect().top
  const minutes = Math.max(0, Math.min(24 * 60 - 15, Math.floor(((y / HOUR_PX) * 60) / 15) * 15))
  emit('create-at', dayStart + minutes * 60000)
}

const scroller = ref(null)
onMounted(() => {
  // 朝 7 時から見えるようにしておく（夜中の作業は上にスクロールすれば見える）
  if (scroller.value) scroller.value.scrollTop = 7 * HOUR_PX
})
</script>

<template>
  <div class="cal" :style="{ '--days': $props.days }">
    <div class="head">
      <div />
      <div v-for="d in days" :key="d.start" class="dayhead" :class="{ today: d.today }">
        <span class="wd">{{ d.weekday }}</span>
        <span class="date num">{{ d.date }}</span>
      </div>
    </div>
    <div ref="scroller" class="body">
      <div class="grid" :style="{ height: `${24 * HOUR_PX}px` }">
        <div class="gutter">
          <div v-for="h in 23" :key="h" class="hour" :style="{ top: `${h * HOUR_PX}px` }">{{ hourLabel(h) }}</div>
        </div>
        <div v-for="(d, i) in days" :key="d.start" class="day" @click="onSlot($event, d.start)">
          <i v-for="h in 23" :key="h" class="line" :style="{ top: `${h * HOUR_PX}px` }" />
          <template v-for="p in byDay[i]" :key="p.plan ? `plan-${p.plan.id}-${p.start}` : `${p.session.id}-${p.start}`">
          <button
            v-if="p.plan"
            class="event plan"
            :class="p.plan.status"
            :style="pieceStyle(p)"
            :title="`${t('plans.onCalendar')}: ${p.plan.name}\n${agentName(p.plan.agent)}\n${planWhen(p)}`"
            @click="emit('open-plan', p.plan.id)"
          >
            <template v-if="visible(p)">
              <!-- 低い枠は 1 行に「題, 時刻」（Google カレンダーと同じ） -->
              <span class="title">{{ p.plan.name }}<template v-if="!roomy(p)">, {{ planWhen(p) }}</template></span>
              <span v-if="roomy(p)" class="sub num">{{ planWhen(p) }}</span>
            </template>
          </button>
          <button
            v-else
            class="event"
            :class="{ selected: p.session.id === selectedId }"
            :style="pieceStyle(p)"
            :title="`${p.session.title || t('untitled')}\n${p.name}${Object.keys(machineLabels).length > 1 ? ` (${machineLabels[p.session.machine] ?? p.session.machine})` : ''}\n${clock(p.start)} – ${clock(p.end)}`"
            @click="emit('select', p.session.id)"
          >
            <template v-if="visible(p)">
              <span class="title">{{ p.session.title || t('untitled') }}</span>
              <span v-if="roomy(p)" class="sub">{{ p.name }}</span>
              <span v-if="roomy(p)" class="sub num">{{ clock(p.start) }} – {{ clock(p.end) }}</span>
            </template>
          </button>
          </template>
          <i v-if="d.today" class="now" :style="{ top: `${px(now - d.start)}px` }" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cal{display:flex; flex-direction:column; height:100%; min-height:0}
.head{display:grid; grid-template-columns:56px repeat(var(--days), 1fr); border-bottom:1px solid var(--rule); padding-bottom:6px}
.dayhead{display:flex; flex-direction:column; align-items:center; gap:2px}
.wd{font-size:11px; font-weight:500; color:var(--muted); text-transform:uppercase; letter-spacing:.08em}
.date{display:grid; place-items:center; width:44px; height:44px; border-radius:50%; font-size:24px; color:var(--ink-soft)}
.dayhead.today .wd{color:var(--accent)}
.dayhead.today .date{background:var(--accent); color:#fff}
.body{overflow-y:auto; flex:1; min-height:0}
.grid{display:grid; grid-template-columns:56px repeat(var(--days), 1fr); position:relative}
.gutter{position:relative}
.hour{position:absolute; right:8px; transform:translateY(-50%); font-size:11px; color:var(--muted)}
.day{position:relative; border-left:1px solid var(--rule); cursor:pointer}
.line{position:absolute; left:0; right:0; border-top:1px solid var(--rule-soft)}
.event{position:absolute; display:flex; flex-direction:column; align-items:flex-start; padding:2px 6px; margin:0; overflow:hidden;
  border:1px solid var(--ground); border-radius:4px; text-align:left; font-size:12px; line-height:1.3}
.event:hover{filter:brightness(.95); z-index:2}
/* 予定（これから動く）は白地に枠線。セッションの塗りと見分ける */
.event.plan{background:var(--ground); color:var(--accent); border:1px dashed var(--accent); z-index:3}
.event.plan:hover{background:var(--accent-faint); filter:none}
.event.plan.failed,.event.plan.missed,.event.plan.skipped{color:var(--warn); border-color:var(--warn)}
.event.selected{box-shadow:0 1px 3px rgba(60,64,67,.3), 0 4px 8px 3px rgba(60,64,67,.15); z-index:3}
.title{font-weight:500; max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.sub{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; opacity:.9}
.now{position:absolute; left:0; right:0; border-top:2px solid var(--now); z-index:4; pointer-events:none}
.now::before{content:""; position:absolute; left:-6px; top:-7px; width:12px; height:12px; border-radius:50%; background:var(--now)}
</style>
