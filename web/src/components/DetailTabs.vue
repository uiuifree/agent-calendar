<script setup>
import { t } from '../i18n.js'

// 右の詳細パネルの上に並べるタブ。押すと切り替え、＋ で固定（ほかのセッションを開いても残す）、✕ で閉じる。
// 固定していないタブ（仮のタブ）は題を斜体にする（次に開いたセッションと入れ替わる）
defineProps({
  tabs: { type: Array, required: true }, // [{ id, kept, tab }]
  active: { type: String, default: null },
  titles: { type: Object, required: true }, // id → 題（読み込めたものだけ）
})
const emit = defineEmits(['select', 'keep', 'close'])
</script>

<template>
  <nav class="strip" role="tablist" :aria-label="t('tabs.label')">
    <div v-for="x in tabs" :key="x.id" class="tab" :class="{ on: x.id === active, kept: x.kept }">
      <button class="name" role="tab" :aria-selected="x.id === active" :title="titles[x.id]" @click="emit('select', x.id)">
        {{ titles[x.id] === undefined ? '…' : titles[x.id] || t('untitled') }}
      </button>
      <button
        class="mini keep"
        :aria-pressed="x.kept"
        :aria-label="x.kept ? t('tabs.unkeep') : t('tabs.keep')"
        :title="x.kept ? t('tabs.unkeep') : t('tabs.keep')"
        @click="emit('keep', x.id)"
      >
        <svg v-if="x.kept" viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
          <path d="M16 3v2h-1v6l2 3v2h-4v5l-1 1-1-1v-5H7v-2l2-3V5H8V3z" />
        </svg>
        <template v-else>＋</template>
      </button>
      <button class="mini" :aria-label="t('tabs.close')" :title="t('tabs.close')" @click="emit('close', x.id)">✕</button>
    </div>
  </nav>
</template>

<style scoped>
.strip{display:flex; flex:none; gap:2px; padding:6px 8px 0; overflow-x:auto; border-left:1px solid var(--rule); border-bottom:1px solid var(--rule); background:var(--panel)}
.tab{display:flex; align-items:center; flex:0 1 220px; min-width:96px; height:32px; padding:0 2px 0 4px; border:1px solid transparent; border-bottom:0; border-radius:8px 8px 0 0; color:var(--muted)}
.tab:hover{background:var(--hover)}
.tab.on{background:var(--ground); border-color:var(--rule); color:var(--ink); margin-bottom:-1px; height:33px}
.name{flex:1; min-width:0; height:100%; border:0; background:none; padding:0 6px; text-align:left; font:inherit; font-size:13px; color:inherit;
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-style:italic}
.tab.kept .name{font-style:normal; font-weight:500}
.mini{display:inline-flex; align-items:center; justify-content:center; flex:none; width:22px; height:22px; padding:0; border:0; border-radius:50%; background:none; color:var(--muted); font-size:12px; line-height:1}
.mini:hover{background:var(--rule-soft); color:var(--ink)}
.mini.keep svg{fill:currentColor}
.tab.kept .mini.keep{color:var(--accent)}
</style>
