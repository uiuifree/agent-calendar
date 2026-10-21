<script setup>
import { computed, nextTick, ref, watch } from 'vue'
import { getBranches, getChanges, getDiff, imageUrl, masked } from '../api.js'
import { t } from '../i18n.js'
import { defaultBase, isImage, patchRows, splitPath } from '../diffview.js'

// セッションの「変更」タブ。GitHub の Files changed と同じく、左にファイルの一覧、右にファイルごとの差分を並べる（見るだけ）。
// 見る対象は、まだ commit していない変更か、このブランチ全体（分かれたところから、いまの作業コピーまで。commit していない分も含む）か、
// セッション中のコミットのどれか
const props = defineProps({
  id: { type: String, required: true },
  commits: { type: Array, default: () => [] }, // [{ hash, subject, … }]
  github: { type: Object, default: null }, // { repo, … }（GitHub のリポジトリなら）
  // 選んでいるコミット（v-model）。概要のコミットを押して開いたときは、そのコミットから見せる
  commit: { type: String, default: '' },
})
const emit = defineEmits(['update:commit'])

// '' = まだ commit していない変更、BRANCH = このブランチ全体、それ以外はコミットのハッシュ
const BRANCH = ':branch'
const source = ref(props.commit)
watch(source, (v) => emit('update:commit', v))
watch(
  () => props.commit,
  (v) => (source.value = v),
)
const dir = ref('')
const files = ref(null)
const diffs = ref({}) // path → { rows, truncated } | { error }
const collapsed = ref({}) // path → true
const failed = ref({}) // `${path}:${side}` → 出せなかったわけ（その側に無い・大きすぎるなど）
const error = ref('')
const loading = ref(false)
// このブランチ全体で比べる相手。候補は作業場所のブランチの一覧（origin のものと手元のもの）。
// 一覧を取れない（設定の組織のリポジトリでないなど）ときは origin/main だけ。既定は origin の既定のブランチ（default）
const bases = ref(null)
const base = ref('')
async function loadBases(d) {
  let origin = null
  try {
    const r = await getBranches(d)
    bases.value = r.branches
    origin = r.default
  } catch {
    bases.value = ['origin/main']
  }
  if (!bases.value.includes(base.value)) base.value = defaultBase(bases.value, origin) ?? ''
}
const against = () => ({ commit: source.value === BRANCH ? null : source.value || null, base: source.value === BRANCH ? base.value : null })

// 差分は並べて取る（ファイルが多くても git を一度に起動しすぎないよう、同時に 6 つまで）。
// 画像は差分の代わりに前と後を並べるので取らない
async function loadDiffs(list, { commit, base }, token) {
  const queue = list.filter((f) => !isImage(f.path))
  const worker = async () => {
    while (queue.length) {
      const f = queue.shift()
      try {
        const r = await getDiff(props.id, f.path, commit, base)
        if (token !== loadToken) return
        diffs.value = { ...diffs.value, [f.path]: { rows: patchRows(r.patch), truncated: r.truncated } }
      } catch (e) {
        if (token !== loadToken) return
        diffs.value = { ...diffs.value, [f.path]: { error: String(e.message ?? e) } }
      }
    }
  }
  await Promise.all(Array.from({ length: 6 }, worker))
}

// 選び直したら、前の対象の読み込みの結果は捨てる
let loadToken = 0
async function load() {
  const token = ++loadToken
  const on = against()
  loading.value = true
  error.value = ''
  files.value = null
  diffs.value = {}
  collapsed.value = {}
  failed.value = {}
  try {
    // このブランチ全体を見るのに、比べる相手がまだ決まっていなければ（作業場所が分かってから）先に決める
    if (source.value === BRANCH && !base.value) {
      if (!dir.value) dir.value = (await getChanges(props.id)).dir
      await loadBases(dir.value)
      if (token !== loadToken) return
      // 比べる相手が 1 つも無ければ取らない（base を付けずに問い合わせると、まだ commit していない差分が返ってしまう）
      if (!base.value) {
        files.value = null
        error.value = t('changes.noBase')
        return
      }
      on.base = base.value
    }
    const r = await getChanges(props.id, on.commit, on.base)
    if (token !== loadToken) return
    dir.value = r.dir
    files.value = r.files
    await loadDiffs(r.files, on, token)
  } catch (e) {
    if (token === loadToken) error.value = String(e.message ?? e)
  } finally {
    if (token === loadToken) loading.value = false
  }
}
// セッションが変わると SessionDetail が部品ごと作り直すので、ここで見るのは選び直しだけ
watch(source, () => load())
watch(base, (v, old) => old && source.value === BRANCH && load())
// 作業場所が分かったら、比べる相手の候補を読んでおく（選択肢の文言に既定の相手の名前を出すため）
watch(dir, (d) => d && !bases.value && loadBases(d))
load()

const totals = computed(() =>
  (files.value ?? []).reduce((a, f) => ({ added: a.added + (f.added ?? 0), removed: a.removed + (f.removed ?? 0) }), { added: 0, removed: 0 }),
)

// 左の一覧で押したファイルの差分へ飛ぶ
const pane = ref(null)
const current = ref(null)
async function jump(path) {
  current.value = path
  collapsed.value = { ...collapsed.value, [path]: false }
  await nextTick()
  pane.value?.querySelector(`[data-path="${CSS.escape(path)}"]`)?.scrollIntoView({ block: 'start', behavior: 'smooth' })
}
// 画像を出せなかったら問い合わせ直して、その側に無い（404）のか、ほかの理由（大きすぎるなど）なのかを出す
async function imageFailed(path, side) {
  const token = loadToken
  const { commit, base: b } = against()
  const url = imageUrl(props.id, path, commit, side, b)
  const r = await fetch(url).catch((e) => e)
  // サーバーのエラー文にはパスが入るので、スクショ用の表示では中身を出さない（api.js の j() と同じ）
  const why =
    r instanceof Response
      ? r.status === 404
        ? t('changes.noImage')
        : masked
          ? `${url.split('?')[0]}: ${r.status}`
          : await r.text()
      : String(r.message ?? r)
  if (token === loadToken) failed.value = { ...failed.value, [`${path}:${side}`]: why || t('changes.imageFailed') }
}
const toggle = (path) => (collapsed.value = { ...collapsed.value, [path]: !collapsed.value[path] })
</script>

<template>
  <div class="changes">
    <div class="bar">
      <select v-model="source" :aria-label="t('changes.source')">
        <option value="">{{ t('changes.working') }}</option>
        <option :value="BRANCH">{{ t('changes.branch', { base: base || 'origin/main' }) }}</option>
        <option v-for="c in commits" :key="c.hash" :value="c.hash">{{ c.hash }} {{ c.subject }}</option>
      </select>
      <select v-if="source === BRANCH && bases?.length" v-model="base" :aria-label="t('changes.base')" :title="t('changes.base')">
        <option v-for="b in bases" :key="b" :value="b">{{ b }}</option>
      </select>
      <span v-if="files" class="muted small">
        {{ t('changes.files', { n: files.length }) }} · <span class="add">+{{ totals.added }}</span> <span class="del">−{{ totals.removed }}</span>
      </span>
      <a v-if="source && source !== BRANCH && github" class="link small" :href="`${github.repo}/commit/${source}`" target="_blank" rel="noopener noreferrer">
        {{ t('changes.onGithub') }} ↗
      </a>
      <span class="spacer" />
      <button class="link small" :disabled="loading" @click="load">{{ t('reload') }}</button>
    </div>
    <p v-if="dir" class="muted small where"><code>{{ dir }}</code></p>
    <p v-if="error" class="chip warn">{{ error }}</p>
    <p v-if="files && !files.length" class="muted">{{ source === BRANCH ? t('changes.branchClean', { base }) : source ? t('changes.emptyCommit') : t('changes.clean') }}</p>

    <div v-if="files?.length" class="split">
      <nav class="tree" :aria-label="t('changes.files', { n: files.length })">
        <button v-for="f in files" :key="f.path" class="entry" :class="{ on: current === f.path }" :title="f.path" @click="jump(f.path)">
          <span class="name">{{ splitPath(f.path).name }}</span>
          <span v-if="f.untracked" class="tag new">{{ t('changes.new') }}</span>
          <span v-else-if="f.added == null" class="muted tiny">{{ t('changes.binary') }}</span>
          <span v-else class="num tiny"><span class="add">+{{ f.added }}</span> <span class="del">−{{ f.removed }}</span></span>
          <!-- 長いときは先頭を省いて末尾（近いフォルダ）を見せる。rtl で末尾の / が前に回らないよう、記号の向きを固定する -->
          <span v-if="splitPath(f.path).dir" class="dir">{{ '\u200e' + splitPath(f.path).dir.slice(0, -1) + '\u200e' }}</span>
        </button>
      </nav>

      <div ref="pane" class="pane">
        <section v-for="f in files" :key="f.path" class="file" :data-path="f.path">
          <header @click="toggle(f.path)">
            <span class="caret">{{ collapsed[f.path] ? '▸' : '▾' }}</span>
            <span class="path">{{ f.path }}</span>
            <span v-if="f.untracked" class="tag new">{{ t('changes.new') }}</span>
            <span v-else-if="f.added == null" class="muted tiny">{{ t('changes.binary') }}</span>
            <span v-else class="num tiny"><span class="add">+{{ f.added }}</span> <span class="del">−{{ f.removed }}</span></span>
          </header>
          <template v-if="!collapsed[f.path]">
            <!-- 画像は GitHub と同じく前と後を並べる。スクショ用の表示では本物が出てしまうので出さない -->
            <p v-if="isImage(f.path) && masked" class="muted small pad">{{ t('changes.imageMasked') }}</p>
            <div v-else-if="isImage(f.path)" class="images">
              <figure v-for="side in ['before', 'after']" :key="side" :class="side">
                <figcaption class="muted tiny">{{ t(`changes.${side}`) }}</figcaption>
                <p v-if="failed[`${f.path}:${side}`]" class="muted small">{{ failed[`${f.path}:${side}`] }}</p>
                <!-- 押すと原寸を別のタブで開く -->
                <a v-else :href="imageUrl(id, f.path, against().commit, side, against().base)" target="_blank" rel="noopener noreferrer">
                  <img
                    :src="imageUrl(id, f.path, against().commit, side, against().base)"
                    :alt="`${t(`changes.${side}`)}: ${f.path}`"
                    loading="lazy"
                    @error="imageFailed(f.path, side)"
                  />
                </a>
              </figure>
            </div>
            <p v-else-if="!diffs[f.path]" class="muted small pad">{{ t('changes.loading') }}</p>
            <p v-else-if="diffs[f.path].error" class="chip warn">{{ diffs[f.path].error }}</p>
            <p v-else-if="!diffs[f.path].rows.length" class="muted small pad">{{ t('changes.noText') }}</p>
            <div v-else class="code">
              <table>
                <tr v-for="(r, i) in diffs[f.path].rows" :key="i" :class="r.kind">
                  <template v-if="r.kind === 'hunk'">
                    <td class="ln" colspan="2" />
                    <td class="text">{{ r.text }}</td>
                  </template>
                  <template v-else>
                    <td class="ln num">{{ r.old ?? '' }}</td>
                    <td class="ln num">{{ r.new ?? '' }}</td>
                    <td class="text"><span class="sign">{{ r.kind === 'add' ? '+' : r.kind === 'del' ? '−' : ' ' }}</span>{{ r.text }}</td>
                  </template>
                </tr>
              </table>
            </div>
            <p v-if="diffs[f.path]?.truncated" class="muted small pad">{{ t('changes.truncated') }}</p>
          </template>
        </section>
      </div>
    </div>
  </div>
</template>

<style scoped>
.changes{display:flex; flex-direction:column; min-height:0; flex:1}
.bar{display:flex; align-items:center; gap:10px; margin:12px 0 4px; flex-wrap:wrap}
.bar select{height:34px; max-width:100%; border:1px solid var(--outline); border-radius:8px; padding:0 8px; background:var(--ground); font:inherit; font-size:13px}
.spacer{flex:1}
.small{font-size:12px}
.tiny{font-size:11px; flex:none}
.where{margin:0 0 8px}
.link{border:0; background:none; padding:0; color:var(--accent); cursor:pointer; text-decoration:none}
.add{color:#1a7f37}
.del{color:#cf222e}
.split{display:grid; grid-template-columns:minmax(160px, 240px) minmax(0, 1fr); gap:12px; flex:1; min-height:0}
.tree{overflow-y:auto; border-right:1px solid var(--rule); padding-right:6px}
.entry{display:grid; grid-template-columns:minmax(0, 1fr) auto; column-gap:6px; align-items:center; width:100%; border:0; background:none;
  padding:5px 6px; border-radius:6px; text-align:left}
.entry:hover{background:var(--hover)}
.entry.on{background:var(--accent-faint)}
.entry .name{font-size:13px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap}
.entry .dir{grid-column:1 / -1; font-size:11px; color:var(--muted); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; direction:rtl; text-align:left}
.tag{font-size:11px; border:1px solid currentColor; border-radius:4px; padding:0 5px}
.tag.new{color:#1a7f37}
.pane{overflow-y:auto; min-width:0}
.file{border:1px solid var(--rule); border-radius:8px; margin-bottom:12px; overflow:hidden}
.file header{position:sticky; top:0; z-index:1; display:flex; align-items:center; gap:8px; padding:6px 10px; background:var(--panel);
  border-bottom:1px solid var(--rule); cursor:pointer}
.caret{flex:none; width:12px; font-size:11px; color:var(--muted)}
.path{flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; font-size:12.5px}
.pad{padding:8px 12px; margin:0}
/* GitHub と同じく、追加は緑・削除は赤の行、見出しは青の行。行番号は左の 2 列 */
.code{overflow-x:auto}
table{border-collapse:collapse; width:100%; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; font-size:12px; line-height:20px}
td{padding:0 8px; vertical-align:top}
.ln{width:1%; min-width:36px; text-align:right; color:var(--muted); user-select:none; white-space:nowrap}
.text{white-space:pre}
.sign{display:inline-block; width:14px; user-select:none}
tr.add td{background:#e6ffec}
tr.add .ln{background:#ccffd8}
tr.del td{background:#ffebe9}
tr.del .ln{background:#ffd7d5}
tr.hunk td{background:#ddf4ff; color:#57606a}
/* 画像: 前は赤、後は緑の枠。透明な部分が分かるよう市松模様の上に置く */
.images{display:grid; grid-template-columns:1fr 1fr; gap:12px; padding:12px}
.images figure{margin:0; min-width:0; text-align:center}
.images figcaption{margin-bottom:6px}
.images img{max-width:100%; max-height:480px; border:1px solid; border-radius:4px;
  background:repeating-conic-gradient(#eee 0 25%, #fff 0 50%) 0 0 / 16px 16px}
.images .before img{border-color:#ff8182}
.images .after img{border-color:#4ac26b}
</style>
