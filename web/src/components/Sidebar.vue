<script setup>
import { computed, ref, watch } from 'vue'
import { getStats } from '../api.js'
import { DAY_MS, fmtHours, fmtUsd, monthGrid } from '../layout.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'

// Google カレンダーの左側と同じ並び: 小さな月のカレンダー・時間の分析情報・ホスト・リポジトリ。
// ピン留めした会話はその上に置く（進行中の会話をすぐ開けるように）
const repoName = (repo) => repo.split('/').filter(Boolean).pop() ?? repo
const ago = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
const props = defineProps({
  anchor: { type: Number, required: true }, // 表示の基準の日
  highlight: { type: Array, default: null }, // 薄く塗る範囲 [from, to)（日・週のとき）
  insightRange: { type: Array, required: true }, // 時間の分析情報で集計する範囲 [from, to)
  now: { type: Number, required: true },
  version: { type: Number, required: true }, // 読み直すたびに増える
  machines: { type: Array, required: true }, // [{ id, label }]
  hosts: { type: Array, default: null }, // 選んだホストの id。null = すべて
  repos: { type: Array, required: true }, // [{ repo, name, slot, count }]（多い順）
  hostCounts: { type: Object, default: () => ({}) }, // ホスト id → 表示中の期間の件数
  hiddenRepos: { type: Array, required: true },
  pins: { type: Array, default: () => [] }, // ピン留めしたセッション（最近動いた順）
  selectedId: { type: String, default: null },
  page: { type: String, default: 'calendar' }, // いまの画面（calendar / stats / plans / repos）
})
const emit = defineEmits(['pick', 'update:hosts', 'update:hiddenRepos', 'more', 'create', 'select', 'go'])

// 画面の切り替え（Google カレンダーの左上と同じく、作成の下に並べる）。キーはショートカット
const PAGES = [
  { id: 'calendar', icon: '▦', key: 'd / w / m' },
  { id: 'stats', icon: '◔', key: 's' },
  { id: 'plans', icon: '⏱', key: 'a' },
  { id: 'repos', icon: '⑂', key: 'r' },
]

// 小さな月のカレンダー。表示中の週に合わせて月を動かし、矢印でも動かせる
const month = ref(new Date(props.anchor))
watch(
  () => props.anchor,
  (a) => {
    const d = new Date(a)
    month.value = new Date(d.getFullYear(), d.getMonth(), 1)
  },
  { immediate: true },
)
const shift = (n) => (month.value = new Date(month.value.getFullYear(), month.value.getMonth() + n, 1))
const title = computed(() => month.value.toLocaleDateString(locale(), { year: 'numeric', month: 'long' }))
const weekdays = computed(() => monthGrid(2026, 9).slice(0, 7).map((d) => new Date(d).toLocaleDateString(locale(), { weekday: 'narrow' })))
const days = computed(() =>
  monthGrid(month.value.getFullYear(), month.value.getMonth()).map((d) => {
    const date = new Date(d)
    return {
      d,
      n: date.getDate(),
      other: date.getMonth() !== month.value.getMonth(),
      today: props.now >= d && props.now < d + DAY_MS,
      inWeek: props.highlight != null && d >= props.highlight[0] && d < props.highlight[1],
    }
  }),
)

// 時間の分析情報（日・週・月の表示に合わせた範囲）
const insights = ref(null)
const rangeLabel = computed(() => {
  const [a, b] = props.insightRange
  if (b - a <= DAY_MS) return new Date(a).toLocaleDateString(locale(), { month: 'short', day: 'numeric', weekday: 'short' })
  return new Intl.DateTimeFormat(locale(), { month: 'short', day: 'numeric' }).formatRange(new Date(a), new Date(b - 1))
})
watch(
  [() => props.insightRange, () => props.version, () => props.hosts],
  async () => {
    try {
      insights.value = await getStats(...props.insightRange, props.hosts)
    } catch {
      insights.value = null
    }
  },
  { immediate: true, deep: true },
)
// 内訳の横棒。リポジトリごとの時間の多い順に 4 つ、残りは「その他」
const bars = computed(() => {
  const rows = (insights.value?.projects ?? []).flatMap((p) => p.repos).sort((a, b) => b.secs - a.secs)
  const sum = rows.reduce((a, r) => a + r.secs, 0)
  if (!sum) return []
  const top = rows.slice(0, 4).map((r) => ({ key: r.repo, name: r.name, color: repoColor(r.slot), pct: (r.secs / sum) * 100 }))
  const rest = rows.slice(4).reduce((a, r) => a + r.secs, 0)
  return rest ? [...top, { key: '*', name: '…', color: '#dadce0', pct: (rest / sum) * 100 }] : top
})

// ホスト（すべて選んでいれば null に戻す）
const hostOn = (id) => props.hosts == null || props.hosts.includes(id)
function toggleHost(id) {
  const all = props.machines.map((m) => m.id)
  const cur = props.hosts ?? all
  const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]
  emit('update:hosts', next.length === all.length && all.every((x) => next.includes(x)) ? null : next)
}

const repoOn = (repo) => !props.hiddenRepos.includes(repo)
const toggleRepo = (repo) =>
  emit('update:hiddenRepos', repoOn(repo) ? [...props.hiddenRepos, repo] : props.hiddenRepos.filter((r) => r !== repo))
</script>

<template>
  <nav class="sidebar">
    <button class="create" @click="emit('create')"><span class="plus">＋</span>{{ t('sidebar.create') }}</button>
    <nav class="pages" :aria-label="t('sidebar.pages')">
      <button
        v-for="p in PAGES"
        :key="p.id"
        class="page"
        :class="{ on: page === p.id }"
        :aria-current="page === p.id ? 'page' : undefined"
        :title="`${t(`pages.${p.id}`)} (${p.key})`"
        @click="emit('go', p.id)"
      >
        <span class="icon" aria-hidden="true">{{ p.icon }}</span>{{ t(`pages.${p.id}`) }}
      </button>
    </nav>
    <section v-if="pins.length" class="pins">
      <h3>{{ t('sidebar.pinned') }}</h3>
      <button v-for="p in pins" :key="p.id" class="pin" :class="{ selected: p.id === selectedId }" @click="emit('select', p.id)">
        <span class="pin-title">{{ p.title || t('untitled') }}</span>
        <span class="pin-meta muted">
          {{ repoName(p.repo) }} · <span class="num">{{ ago(p.last_ts) }}</span>
          <span v-if="p.status" class="chip" :class="p.status">{{ t(`status.${p.status}`) }}</span>
        </span>
      </button>
    </section>
    <section class="mini">
      <div class="mini-head">
        <span>{{ title }}</span>
        <span>
          <button class="icon-btn small" aria-label="‹" @click="shift(-1)">‹</button>
          <button class="icon-btn small" aria-label="›" @click="shift(1)">›</button>
        </span>
      </div>
      <div class="grid">
        <span v-for="(w, i) in weekdays" :key="`w${i}`" class="wd">{{ w }}</span>
        <button
          v-for="day in days"
          :key="day.d"
          class="day num"
          :class="{ other: day.other, today: day.today, week: day.inWeek }"
          @click="emit('pick', day.d)"
        >
          {{ day.n }}
        </button>
      </div>
    </section>

    <section>
      <h3>{{ t('sidebar.insights') }}</h3>
      <div class="muted small">{{ rangeLabel }}</div>
      <template v-if="insights">
        <div class="total">
          {{ t('stats.activeTime') }} <b class="num">{{ fmtHours(insights.total.secs) }}</b> · <span class="num">{{ fmtUsd(insights.total.usd) }}</span>
        </div>
        <div class="bar" aria-hidden="true">
          <i v-for="b in bars" :key="b.key" :style="{ width: `${b.pct}%`, background: b.color }" :title="b.name" />
        </div>
      </template>
      <button class="btn more" @click="emit('more')">✦ {{ t('sidebar.moreInsights') }}</button>
    </section>

    <section v-if="machines.length > 1">
      <h3>{{ t('sidebar.hosts') }}</h3>
      <label v-for="m in machines" :key="m.id" class="check">
        <input type="checkbox" :checked="hostOn(m.id)" @change="toggleHost(m.id)" />
        <span class="name">{{ m.label }}</span>
        <span class="muted num">{{ hostCounts[m.id] ?? 0 }}</span>
      </label>
    </section>

    <section v-if="repos.length">
      <h3>{{ t('sidebar.repos') }}</h3>
      <label v-for="r in repos" :key="r.repo" class="check" :title="r.repo">
        <input type="checkbox" :checked="repoOn(r.repo)" :style="{ accentColor: repoColor(r.slot) }" @change="toggleRepo(r.repo)" />
        <span class="name">{{ r.name }}</span>
        <span class="muted num">{{ r.count }}</span>
      </label>
    </section>
  </nav>
</template>

<style scoped>
.sidebar{width:256px; padding:8px 12px 24px 16px; overflow-y:auto; min-height:0}
/* Google カレンダーの「作成」ボタンと同じ、影付きの丸いボタン */
.create{display:inline-flex; align-items:center; gap:8px; height:36px; padding:0 16px 0 12px; margin:2px 0 12px; border:0; border-radius:18px;
  background:var(--ground); color:var(--ink); font-size:14px; font-weight:500;
  box-shadow:0 1px 2px rgba(60,64,67,.3), 0 1px 3px 1px rgba(60,64,67,.15)}
.create:hover{background:var(--accent-faint); box-shadow:0 1px 3px rgba(60,64,67,.3), 0 4px 8px 3px rgba(60,64,67,.15)}
.create .plus{font-size:18px; line-height:1; color:var(--accent)}
section{margin-bottom:20px}
.pages{display:flex; flex-direction:column; gap:2px; margin:0 0 16px}
.page{display:flex; align-items:center; gap:12px; height:36px; padding:0 12px; border:0; border-radius:18px; background:none;
  font-size:14px; color:var(--ink); text-align:left}
.page:hover{background:var(--hover)}
.page.on{background:var(--accent-soft); color:#041e49; font-weight:500}
.page .icon{width:18px; text-align:center; color:var(--ink-soft)}
.page.on .icon{color:var(--accent)}
.pins{margin-top:16px}
.pin{display:flex; flex-direction:column; align-items:flex-start; width:100%; border:0; background:none; padding:6px 8px; border-radius:8px; text-align:left; min-width:0}
.pin:hover{background:var(--hover)}
.pin.selected{background:var(--accent-faint)}
.pin-title{max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; font-weight:500; color:var(--ink)}
.pin-meta{display:flex; align-items:center; gap:4px; font-size:12px; max-width:100%; overflow:hidden; white-space:nowrap}
.pin-meta .chip{font-size:11px; padding:0 6px}
h3{font-size:14px; font-weight:500; margin:0 0 8px; color:var(--ink)}
.small{font-size:12px}
.mini-head{display:flex; align-items:center; justify-content:space-between; font-size:14px; font-weight:500; padding-left:6px}
.icon-btn.small{width:28px; height:28px; font-size:16px}
.grid{display:grid; grid-template-columns:repeat(7, 1fr); gap:2px 0; margin-top:4px}
.wd{text-align:center; font-size:10px; font-weight:500; color:var(--muted); line-height:24px}
.day{height:28px; border:0; background:none; border-radius:14px; font-size:11px; color:var(--ink)}
.day.other{color:var(--muted)}
.day.week{background:var(--accent-faint); border-radius:0}
.day.week:nth-child(7n+1){border-radius:14px 0 0 14px}
.day.week:nth-child(7n){border-radius:0 14px 14px 0}
.day:hover{background:var(--hover)}
.day.today{background:var(--accent); color:#fff; border-radius:14px; font-weight:500}
.total{margin:6px 0; font-size:13px}
.bar{display:flex; height:10px; border-radius:5px; overflow:hidden; background:var(--rule-soft); margin-bottom:12px}
.bar i{display:block; height:100%}
.more{width:100%; justify-content:center; color:var(--accent)}
.check{display:flex; align-items:center; gap:10px; height:32px; padding:0 4px; border-radius:16px; cursor:pointer; font-size:14px}
.check:hover{background:var(--hover)}
.check input{width:18px; height:18px; margin:0; accent-color:var(--accent)}
.check .name{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.check .muted{font-size:12px}
</style>
