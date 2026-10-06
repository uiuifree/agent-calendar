<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { getBranches, getModels, startSession } from '../api.js'
import { useEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'
import { MAX_IMAGES, pickImages, toPayload } from '../images.js'
import { rememberedMode } from '../remembered.js'
import { renderMarkdown } from '../markdown.js'
import PermissionAsk from './PermissionAsk.vue'

// リポジトリで新しいセッションを始める。始まったら（セッションの ID が届いたら）ダイアログを閉じて、
// カレンダーでセッションを押したときと同じく右のパネルに会話を開く。実行は裏で続き、会話の画面で進み具合と許可の確認を見る
const props = defineProps({
  repo: { type: Object, required: true }, // { name, local, … }
})
const emit = defineEmits(['close', 'opened', 'ran', 'failed'])
useEscape(() => emit('close'))

const agent = ref('claude')
const mode = rememberedMode() // 許可の範囲。既定は自動判定で、前回選んだものを覚えている
const prompt = ref('')
// 使うモデル。空なら CLI の既定。候補はこのマシンの記録に出てきたモデル（最近使った順）。取れなくても既定のまま始められる
const model = ref('')
const models = ref({}) // { claude: { cli_default, models }, codex: … }
onMounted(async () => {
  try {
    models.value = await getModels()
  } catch {
    models.value = {}
  }
})
// エージェントを切り替えたら消す（Claude のモデル名を Codex に渡さない）
watch(agent, () => (model.value = ''))
const running = ref(false)
const live = ref([])
const sessionId = ref(null)
// 別の作業場所（git worktree）で始める。新しいブランチで切るか、すでにあるものを選ぶ（existing はそのフォルダ。空なら新しく切る）。
// どこで始めたかは最初の 1 行で届く
const worktree = ref(false)
const existing = ref('')
const branch = ref('')
// どのブランチから切るか。空は origin の既定のブランチ。
// 一覧は手元にある記録から（origin のものと手元のもの）。取れなくても既定のまま始められる
const base = ref('')
const branches = ref(null) // { default, branches, worktrees }
// 既定のブランチの名前。GitHub の一覧で分かっている名前が正（手元の origin/HEAD は clone したときのままで、
// GitHub で既定を変えても追従しない）。一覧に無いときだけ手元の記録を使う
const defaultBranch = computed(() => props.repo.default_branch || branches.value?.default || '')
// 既定のブランチを一覧と同じ表記で（origin にあれば origin/名前）。画面に出す名前と実際に切る元を揃える
const defaultEntry = computed(() => {
  const name = defaultBranch.value
  return name && branches.value?.branches.includes(`origin/${name}`) ? `origin/${name}` : name
})
watch(worktree, async (on) => {
  if (!on || branches.value) return
  try {
    branches.value = await getBranches(props.repo.local)
  } catch {
    branches.value = { default: null, branches: [], worktrees: [] }
  }
})
const workspace = ref(null) // { path, branch }
const finished = ref(false)
const canRun = computed(() => !running.value && !finished.value && prompt.value.trim().length > 0)

// 画像（会話の入力欄と同じく、貼り付けとドロップで足す）
const attachments = ref([])
const attachError = ref('')
function addImages(files) {
  const { accepted, error } = pickImages([...files], attachments.value.length)
  attachError.value = error ? t(error, { n: MAX_IMAGES }) : ''
  attachments.value.push(...accepted.map((file) => ({ file, url: URL.createObjectURL(file) })))
  return accepted.length > 0
}
function onPaste(e) {
  if (addImages(e.clipboardData?.files ?? [])) e.preventDefault()
}
function removeImage(i) {
  URL.revokeObjectURL(attachments.value[i].url)
  attachments.value.splice(i, 1)
}
onUnmounted(() => {
  for (const a of attachments.value) URL.revokeObjectURL(a.url)
})

async function run() {
  if (!canRun.value) return
  running.value = true
  live.value = []
  try {
    const images = await Promise.all(attachments.value.map((a) => toPayload(a.file)))
    const input = {
      dir: props.repo.local,
      agent: agent.value,
      mode: mode.value,
      prompt: prompt.value.trim(),
      images,
      worktree: worktree.value,
      branch: branch.value.trim(),
      // 既定のブランチも名前で渡す（空のままだと、手元に origin/HEAD の記録が無い clone では HEAD から切られる）
      base: base.value || defaultEntry.value,
      existing: worktree.value ? existing.value : '',
      model: model.value.trim(),
    }
    await startSession(input, (ev) => {
      if (ev.kind === 'session') {
        sessionId.value = ev.id
        emit('opened', ev.id)
      }
      else if (ev.kind === 'worktree') workspace.value = ev
      else live.value.push(ev)
    })
  } catch (e) {
    live.value.push({ kind: 'done', ok: false, text: String(e.message ?? e) })
  } finally {
    running.value = false
    finished.value = true
    // 会話の画面へ移ったあと（ダイアログは隠れている）に失敗したら、理由を親に渡して見えるところに出す
    const end = live.value.at(-1)
    if (sessionId.value && end?.kind === 'done' && !end.ok && !end.stopped) emit('failed', end.text ?? '')
    emit('ran')
  }
}

function onKey(e) {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    run()
  }
}
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="dialog" role="dialog" aria-modal="true" :aria-label="t('repos.startTitle', { name: repo.name })" :lang="locale()" @dragover.prevent @drop.prevent="addImages($event.dataTransfer?.files ?? [])">
      <h2>{{ t('repos.startTitle', { name: repo.name }) }}</h2>
      <p class="muted small"><code>{{ repo.local }}</code></p>
      <div class="row">
        <select v-model="agent" :disabled="running || finished" :aria-label="t('plans.form.agent')">
          <option value="claude">{{ t('source.claude') }}</option>
          <option value="codex">{{ t('source.codex') }}</option>
        </select>
        <select v-model="mode" :disabled="running || finished" :aria-label="t('plans.form.mode')">
          <option value="read">{{ t('conv.modeRead') }}</option>
          <option value="edit">{{ t('conv.modeEdit') }}</option>
          <option value="auto">{{ t('conv.modeAuto') }}</option>
        </select>
        <select v-model="model" class="model" :disabled="running || finished" :aria-label="t('repos.model')">
          <option value="">{{ models[agent]?.cli_default ? t('repos.modelDefault', { name: models[agent].cli_default }) : t('repos.modelDefaultUnknown') }}</option>
          <option v-for="m in models[agent]?.models ?? []" :key="m" :value="m">{{ m }}</option>
        </select>
      </div>
      <label class="check">
        <input v-model="worktree" type="checkbox" :disabled="running || finished" />
        {{ t('repos.worktree') }}
      </label>
      <label v-if="worktree" class="base">
        {{ t('repos.worktreePick') }}
        <select v-model="existing" :disabled="running || finished">
          <option value="">{{ t('repos.worktreeNew') }}</option>
          <option v-for="w in branches?.worktrees ?? []" :key="w.path" :value="w.path">{{ w.branch || t('repos.detached') }} — {{ w.path }}</option>
        </select>
      </label>
      <label v-if="worktree && !existing" class="base">
        {{ t('repos.base') }}
        <select v-model="base" :disabled="running || finished">
          <option value="">{{ defaultEntry ? t('repos.baseDefault', { name: defaultEntry }) : t('repos.baseHead') }}</option>
          <option v-for="b in branches?.branches ?? []" :key="b" :value="b">{{ b }}</option>
        </select>
      </label>
      <input
        v-if="worktree && !existing"
        v-model="branch"
        class="branch"
        type="text"
        spellcheck="false"
        :placeholder="t('repos.branchPlaceholder')"
        :aria-label="t('repos.branch')"
        :disabled="running || finished"
      />
      <textarea v-model="prompt" rows="4" :placeholder="t('plans.form.prompt')" :disabled="running || finished" @keydown="onKey" @paste="onPaste" />
      <div v-if="attachments.length" class="thumbs">
        <span v-for="(a, i) in attachments" :key="a.url" class="thumb">
          <img :src="a.url" alt="" />
          <button v-if="!running && !finished" class="x" :aria-label="t('conv.removeImage')" @click="removeImage(i)">✕</button>
        </span>
      </div>
      <p v-if="attachError" class="chip warn">{{ attachError }}</p>
      <p class="muted small">{{ t('repos.startNote') }} {{ t('conv.attach', { n: MAX_IMAGES }) }}</p>

      <p v-if="workspace" class="ws">{{ workspace.base ? t('repos.workspace', { branch: workspace.branch, base: workspace.base }) : t('repos.workspaceExisting', { branch: workspace.branch || t('repos.detached') }) }}<br /><code>{{ workspace.path }}</code></p>
      <div v-if="live.length || running" class="live">
        <template v-for="(ev, i) in live" :key="i">
          <div v-if="ev.kind === 'text'" class="md" v-html="renderMarkdown(ev.text)" />
          <div v-else-if="ev.kind === 'tool'" class="tool"><span class="name">{{ ev.name }}</span> {{ ev.text }}</div>
          <PermissionAsk v-else-if="ev.kind === 'permission'" :ask="ev" />
          <p v-else-if="ev.kind === 'done' && ev.stopped" class="chip">{{ t('conv.stopped') }}</p>
          <p v-else-if="ev.kind === 'done' && !ev.ok" class="chip warn">{{ t('conv.failed', { e: ev.text ?? '' }) }}</p>
        </template>
        <p v-if="running" class="muted small">{{ t('repos.running') }}</p>
      </div>

      <div class="actions">
        <button class="btn" @click="emit('close')">{{ t('repos.close') }}</button>
        <button class="btn primary" :disabled="!canRun" @click="run">{{ running ? t('repos.running') : t('repos.run') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.backdrop{position:fixed; inset:0; background:rgba(32,33,36,.4); display:grid; place-items:center; z-index:20}
.dialog{width:min(640px, calc(100vw - 32px)); max-height:calc(100vh - 48px); overflow-y:auto; background:var(--ground); border-radius:28px; padding:24px;
  box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
h2{font-size:22px; font-weight:400; margin:0 0 4px}
.small{font-size:12px}
.row{display:flex; gap:8px; margin:12px 0 8px}
.check{display:flex; align-items:center; gap:8px; font-size:13px; margin:0 0 8px}
.model{flex:1; min-width:0}
.base{display:flex; align-items:center; gap:8px; font-size:13px; margin:0 0 8px}
.base select{flex:1; min-width:0}
.branch{width:100%; height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 10px; font:inherit; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; font-size:13px; margin-bottom:8px}
.ws{margin:12px 0 0; font-size:13px; color:var(--accent)}
.ws code{font-size:12px; color:var(--ink-soft)}
select{height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 8px; background:var(--ground)}
textarea{width:100%; resize:vertical; border:1px solid var(--outline); border-radius:8px; padding:8px 10px; font:inherit; background:var(--ground)}
textarea:focus{outline:2px solid var(--accent); border-color:transparent}
.thumbs{display:flex; flex-wrap:wrap; gap:8px; margin-top:8px}
.thumbs img{display:block; width:72px; height:72px; object-fit:cover; border:1px solid var(--rule); border-radius:8px}
.thumb{position:relative}
.thumb .x{position:absolute; top:-6px; right:-6px; width:20px; height:20px; padding:0; border:1px solid var(--rule); border-radius:50%;
  background:var(--ground); font-size:11px; line-height:1; color:var(--ink-soft)}
.live{margin-top:12px; padding:10px 12px; border:1px solid var(--rule-soft); border-radius:12px; background:var(--panel); max-height:320px; overflow-y:auto}
.md{line-height:1.75; word-break:break-word; margin:8px 0; white-space:normal}
.md :deep(p){margin:.4em 0}
.md :deep(pre){overflow-x:auto}
.tool{font-size:12px; color:var(--ink-soft); font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.tool .name{font-weight:500; color:var(--ink)}
.actions{display:flex; justify-content:flex-end; gap:8px; margin-top:16px}
</style>
