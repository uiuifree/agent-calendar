<script setup>
import { onMounted, ref } from 'vue'
import { deleteBranch, fetchRepo, getRepoManage, removeWorktree } from '../api.js'
import { useEscape } from '../dialog.js'
import { locale, t } from '../i18n.js'

// リポジトリごとの小さな操作: GitHub からの取り直しと、使い終わった作業場所（git worktree）・手元のブランチの片付け。
// 手元のリポジトリだけを触る。変更が残っているものは git が断るので、その理由をそのまま見せる
const props = defineProps({
  repo: { type: Object, required: true }, // { name, local, … }
})
const emit = defineEmits(['close', 'changed', 'open-session'])
useEscape(() => emit('close'))

const data = ref(null) // { worktrees, branches, base }
const error = ref('')
const note = ref('')
const busy = ref(false)

async function load() {
  try {
    data.value = await getRepoManage(props.repo.local)
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}
onMounted(load)

// 操作を 1 つ動かして、一覧を読み直す。断られたら理由を出す。戻り値は断られた理由（通れば null）
async function act(op) {
  busy.value = true
  error.value = ''
  note.value = ''
  try {
    await op()
    emit('changed')
    return null
  } catch (e) {
    return String(e.message ?? e)
  } finally {
    await load()
    busy.value = false
  }
}

async function fetchNow() {
  let report = ''
  const refused = await act(async () => {
    report = (await fetchRepo(props.repo.local)).report
  })
  if (refused) error.value = refused
  else note.value = report || t('repos.menu.fetched')
}

async function remove(w) {
  if (!window.confirm(t('repos.menu.removeConfirm', { path: w.path }))) return
  const refused = await act(() => removeWorktree(props.repo.local, w.path))
  if (refused) error.value = refused
}

async function del(b) {
  if (!window.confirm(t('repos.menu.deleteConfirm', { branch: b.branch }))) return
  const refused = await act(() => deleteBranch(props.repo.local, b.branch, false))
  if (!refused) return
  // まだ取り込まれていない commit があるブランチ（squash で取り込んだものもこう見える）は、確かめてから消す
  if (refused.includes('not fully merged') && window.confirm(t('repos.menu.forceConfirm', { branch: b.branch }))) {
    const again = await act(() => deleteBranch(props.repo.local, b.branch, true))
    if (again) error.value = again
    return
  }
  error.value = refused
}

const when = (ms) => new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' })
const mergedTitle = () => (data.value?.base ? t('repos.menu.mergedInto', { base: data.value.base }) : null)

// 消せない理由（消せるなら空）
const worktreeBlock = (w) =>
  w.running ? t('repos.menu.running') : w.schedule != null ? t('repos.menu.usedBySchedule', { id: w.schedule }) : ''
</script>

<template>
  <div class="backdrop" @click.self="emit('close')">
    <div class="dialog" role="dialog" aria-modal="true" :aria-label="t('repos.menu.title', { name: repo.name })" :lang="locale()">
      <h2>{{ t('repos.menu.title', { name: repo.name }) }}</h2>
      <p class="muted small"><code>{{ repo.local }}</code></p>
      <div class="line">
        <button class="btn small" :disabled="busy" @click="fetchNow">{{ t('repos.menu.fetch') }}</button>
        <span class="muted small">{{ t('repos.menu.fetchNote') }}</span>
      </div>
      <p v-if="error" class="chip warn msg">{{ error }}</p>
      <pre v-if="note" class="note">{{ note }}</pre>

      <template v-if="data">
        <h3>{{ t('repos.menu.worktrees') }} <span class="muted num">{{ data.worktrees.length }}</span></h3>
        <p v-if="!data.worktrees.length" class="muted small">{{ t('repos.menu.noWorktrees') }}</p>
        <div v-for="w in data.worktrees" :key="w.path" class="item">
          <span class="what">
            <span class="name">{{ w.branch || t('repos.detached') }}</span>
            <code>{{ w.path }}</code>
            <!-- 消してよいかの目安: 残っている変更・取り込み済みか・最後のセッション -->
            <span class="facts small">
              <span v-if="w.changes != null" :class="w.changes ? 'chip talk' : 'muted'">{{ w.changes ? t('repos.menu.changes', { n: w.changes }) : t('repos.menu.clean') }}</span>
              <span v-if="w.merged" class="chip done" :title="mergedTitle()">{{ t('repos.menu.merged') }}</span>
              <button v-if="w.last_session" class="link" @click="emit('open-session', w.last_session.id)">
                {{ t('repos.menu.lastSession', { when: when(w.last_session.last_ts) }) }}
              </button>
            </span>
            <span v-if="worktreeBlock(w)" class="muted small">{{ worktreeBlock(w) }}</span>
          </span>
          <button class="btn small" :disabled="busy || !!worktreeBlock(w)" @click="remove(w)">{{ t('repos.menu.remove') }}</button>
        </div>

        <h3>{{ t('repos.menu.branches') }} <span class="muted num">{{ data.branches.length }}</span></h3>
        <div v-for="b in data.branches" :key="b.branch" class="item">
          <span class="what">
            <span class="name">{{ b.branch }} <span v-if="b.merged" class="chip done" :title="mergedTitle()">{{ t('repos.menu.merged') }}</span></span>
            <span v-if="b.checked_out" class="muted small">{{ t('repos.menu.checkedOut', { path: b.checked_out }) }}</span>
          </span>
          <button class="btn small" :disabled="busy || !!b.checked_out" @click="del(b)">{{ t('repos.menu.delete') }}</button>
        </div>
        <p class="muted small">{{ t('repos.menu.localOnly') }}</p>
      </template>

      <div class="actions">
        <button class="btn" @click="emit('close')">{{ t('repos.close') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.backdrop{position:fixed; inset:0; background:rgba(32,33,36,.4); display:grid; place-items:center; z-index:20}
.dialog{width:min(720px, calc(100vw - 32px)); max-height:calc(100vh - 48px); overflow-y:auto; background:var(--ground); border-radius:28px; padding:24px;
  box-shadow:0 4px 8px 3px rgba(60,64,67,.15), 0 1px 3px rgba(60,64,67,.3)}
h2{font-size:22px; font-weight:400; margin:0 0 4px}
h3{font-size:14px; font-weight:500; margin:20px 0 8px}
.small{font-size:12px}
.btn.small{height:30px; padding:0 12px; font-size:13px; flex:none}
.line{display:flex; align-items:center; gap:12px; margin-top:12px}
.msg{margin:12px 0 0; white-space:pre-wrap}
.note{margin:12px 0 0; padding:8px 10px; border:1px solid var(--rule-soft); border-radius:8px; background:var(--panel); font-size:12px; white-space:pre-wrap; max-height:160px; overflow-y:auto}
.item{display:flex; align-items:center; justify-content:space-between; gap:12px; padding:8px 0; border-top:1px solid var(--rule-soft)}
.what{display:flex; flex-direction:column; gap:2px; min-width:0}
.what code{font-size:12px; color:var(--ink-soft); overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.name{font-size:13px; font-weight:500; word-break:break-all}
.facts{display:flex; flex-wrap:wrap; align-items:center; gap:6px}
.facts .chip,.name .chip{height:20px; font-size:11px; padding:0 6px}
.link{border:0; background:none; padding:0; color:var(--accent); font:inherit; cursor:pointer; text-decoration:underline; text-underline-offset:.2em}
.actions{display:flex; justify-content:flex-end; gap:8px; margin-top:16px}
</style>
