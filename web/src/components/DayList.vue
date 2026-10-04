<script setup>
import { computed } from 'vue'
import { DAY_MS, segments, splitByDay, sumUsd, fmtHours, fmtUsd } from '../layout.js'
import { repoColor } from '../colors.js'
import { locale, t } from '../i18n.js'

// 日表示の右側: その日のセッションを時刻順に並べ、上にその日の作業時間と金額を出す
const props = defineProps({
  from: { type: Number, required: true }, // その日の 0:00
  sessions: { type: Array, required: true },
  repos: { type: Object, required: true },
  machineLabels: { type: Object, default: null }, // ホストが複数あるときだけ
  selectedId: { type: String, default: null },
})
const emit = defineEmits(['select'])

const clock = (ms) => new Date(ms).toLocaleTimeString(locale(), { hour: 'numeric', minute: '2-digit' })

const day = computed(() => {
  const end = props.from + DAY_MS
  const entries = []
  const cells = new Set()
  for (const s of props.sessions) {
    const pieces = segments(s.buckets).flatMap((seg) => splitByDay(seg, props.from, 1))
    if (!pieces.length) continue
    for (const p of pieces) for (const c of p.cells) cells.add(c.t)
    const info = props.repos[s.repo] ?? { name: s.repo, slot: null }
    entries.push({
      session: s,
      name: info.name,
      color: repoColor(info.slot),
      start: Math.min(...pieces.map((p) => p.start)),
      end: Math.max(...pieces.map((p) => p.end)),
      usd: sumUsd(s.usd, props.from, end),
    })
  }
  entries.sort((a, b) => a.start - b.start)
  // 作業時間は、並行して開いていたセッションの枠を合わせてから数える
  const secs = segments([...cells].map((c) => [c / 1000, 1])).reduce((a, s) => a + (s.end - s.start) / 1000, 0)
  return { entries, secs, usd: entries.reduce((a, e) => a + e.usd, 0) }
})
</script>

<template>
  <div class="daylist">
    <div class="sum muted num">{{ t('work', { hours: fmtHours(day.secs) }) }} · {{ fmtUsd(day.usd) }}</div>
    <ol class="list">
      <li v-for="e in day.entries" :key="e.session.id" :class="{ selected: e.session.id === selectedId }">
        <button class="row" @click="emit('select', e.session.id)">
          <i class="dot" :style="{ background: e.color }" />
          <span class="main">
            <span class="title">{{ e.session.title || t('untitled') }}</span>
            <span class="meta">
              <span class="num">{{ clock(e.start) }} – {{ clock(e.end) }}</span> · {{ e.name }}
            </span>
          </span>
          <span class="tags">
            <span v-if="machineLabels" class="chip host">{{ machineLabels[e.session.machine] ?? e.session.machine }}</span>
            <span v-if="e.session.source === 'codex'" class="chip src">{{ t('source.codex') }}</span>
            <span v-if="e.session.status" class="chip" :class="e.session.status">{{ t(`status.${e.session.status}`) }}</span>
          </span>
          <span class="usd num" :title="e.session.unpriced ? t('unpricedHint') : t('apiPrice')">{{ fmtUsd(e.usd) }}{{ e.session.unpriced ? '+' : '' }}</span>
        </button>
      </li>
    </ol>
    <p v-if="!day.entries.length" class="muted">{{ t('nothing') }}</p>
  </div>
</template>

<style scoped>
.daylist{min-width:0; overflow-y:auto; padding:4px 0 24px 16px; border-left:1px solid var(--rule)}
.sum{font-size:13px; margin:4px 8px 8px}
.list{list-style:none; margin:0; padding:0}
.list li{border-radius:8px}
.list li:hover{background:var(--hover)}
.list li.selected{background:var(--accent-faint)}
.row{display:grid; grid-template-columns:10px minmax(0, 1fr) auto 56px; gap:10px; align-items:center; width:100%;
  border:0; background:none; padding:6px 8px; text-align:left}
.dot{display:inline-block; width:10px; height:10px; border-radius:50%}
.main{display:flex; flex-direction:column; min-width:0}
.title{font-weight:500; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.meta{font-size:12px; color:var(--muted); overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.tags{display:flex; gap:4px; flex-wrap:wrap; justify-content:flex-end}
.chip.host{color:var(--ink); border-color:var(--outline); background:var(--hover)}
.usd{text-align:right; color:var(--ink-soft); font-size:13px}
</style>
