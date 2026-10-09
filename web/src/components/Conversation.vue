<script setup>
import { computed, nextTick, onUnmounted, ref, watch } from 'vue'
import { getAsks, getTranscript, sendInstruction, stopInstruction } from '../api.js'
import { locale, t } from '../i18n.js'
import { renderMarkdown } from '../markdown.js'
import { MAX_IMAGES, pickImages, toPayload } from '../images.js'
import { rememberedMode } from '../remembered.js'
import PermissionAsk from './PermissionAsk.vue'

// セッションの会話。履歴をチャットの形で並べ、下の入力欄から続きの指示を送る
const props = defineProps({
  id: { type: String, required: true },
  // 送れないときの理由（null なら送れる）
  blocked: { type: String, default: null },
  // ほかの画面（「ここで始める」や別のタブ）から実行中。数秒おきに履歴を読み直して進み具合を見せる
  following: { type: Boolean, default: false },
  // 別のマシンで動いたセッション。ここからは送れないので入力欄を出さない
  remote: { type: Boolean, default: false },
  // セッションの最後の記録の時刻。変わったら履歴を読み直す（隠れていたタブに戻ったとき、見ていないあいだに進んだ分を出す）
  stamp: { type: Number, default: 0 },
})
const emit = defineEmits(['sent'])

const items = ref([])
const total = ref(0)
const start = ref(0)
const error = ref('')
const list = ref(null)

const clock = (ms) => (ms ? new Date(ms).toLocaleString(locale(), { month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' }) : '')

async function load(scroll = true) {
  error.value = ''
  try {
    const r = await getTranscript(props.id)
    // 古い履歴を読み足していたら、その分は残す（読み直しで消さない）
    if (!scroll && start.value < r.start && items.value.length) {
      items.value = [...items.value.slice(0, r.start - start.value), ...r.items]
    } else {
      items.value = r.items
      start.value = r.start
    }
    total.value = r.total
    if (scroll) {
      await nextTick()
      list.value?.scrollTo({ top: list.value.scrollHeight })
    }
  } catch (e) {
    error.value = String(e)
  }
}

// 読み込み中に何度押しても 1 回だけ（同じページを二重に足さないように）
const loadingOlder = ref(false)
async function older() {
  if (loadingOlder.value) return
  loadingOlder.value = true
  try {
    const r = await getTranscript(props.id, start.value)
    const before = list.value?.scrollHeight ?? 0
    items.value = [...r.items, ...items.value]
    start.value = r.start
    await nextTick()
    // 読み足しても今見ている位置がずれないように
    if (list.value) list.value.scrollTop += list.value.scrollHeight - before
  } catch (e) {
    error.value = String(e)
  } finally {
    loadingOlder.value = false
  }
}

watch(() => props.id, () => load(), { immediate: true })

// 実行中は履歴を読み直す。いちばん下を見ているときだけ、新しい発言に合わせて下へ送る
let follow = null
async function refresh() {
  const el = list.value
  const atBottom = !el || el.scrollHeight - el.scrollTop - el.clientHeight < 80
  await load(false)
  if (atBottom) await scrollDown()
}
watch(
  () => props.following,
  (on, was) => {
    clearInterval(follow)
    follow = on ? setInterval(refresh, 3000) : null
    // 終わったら最後まで読み直す（入力欄はそのままにして、書きかけを残す）。ここから送ったものは send が読み直す
    if (!on && was && !running.value) load()
  },
  { immediate: true },
)

// 送る
const prompt = ref('')
// ここから送っている最中は読み直さない（終わったときに読み直す）。書きかけの指示はそのまま残る
watch(
  () => props.stamp,
  () => running.value || refresh(),
)
const mode = rememberedMode() // 許可の範囲。既定は自動判定で、前回選んだものを覚えている
const running = ref(false)
const live = ref([]) // 実行中に届いた途中経過
const canSend = computed(() => !props.blocked && !running.value && prompt.value.trim().length > 0)

// 付ける画像。貼り付け（Ctrl+V）とドロップで足し、× で外す。送ったら空にする
const attachments = ref([]) // [{ file, url }]
const attachError = ref('')
function addImages(files) {
  const { accepted, error } = pickImages([...files], attachments.value.length)
  attachError.value = error ? t(error, { n: MAX_IMAGES }) : ''
  attachments.value.push(...accepted.map((file) => ({ file, url: URL.createObjectURL(file) })))
  return accepted.length > 0
}
function onPaste(e) {
  // 画像が無ければ文字の貼り付けはそのまま
  if (addImages(e.clipboardData?.files ?? [])) e.preventDefault()
}
function onDrop(e) {
  addImages(e.dataTransfer?.files ?? [])
}
function removeImage(i) {
  URL.revokeObjectURL(attachments.value[i].url)
  attachments.value.splice(i, 1)
}
function clearImages() {
  for (const a of attachments.value) URL.revokeObjectURL(a.url)
  attachments.value = []
  attachError.value = ''
}
watch(() => props.id, clearImages)
onUnmounted(clearImages)

// 答えを待っている許可の問い合わせ。画面を切り替えたり別のタブから送ったりして、途中経過の流れを
// 持っていないときも答えられるように、開いているあいだは数秒おきに見に行く（いま流れている分は除く）
const waiting = ref([])
async function pollAsks() {
  try {
    const shown = new Set(live.value.filter((e) => e.kind === 'permission').map((e) => e.request_id))
    waiting.value = (await getAsks(props.id)).asks.filter((a) => !shown.has(a.request_id))
  } catch {
    // 見に行けなくても会話は読める
  }
}
const askTimer = setInterval(pollAsks, 3000)
watch(() => props.id, pollAsks, { immediate: true })
onUnmounted(() => {
  clearInterval(askTimer)
  clearInterval(follow)
})

async function send() {
  if (!canSend.value) return
  const text = prompt.value.trim()
  const chosen = mode.value // 実行中に選び直しても、送った指示の許可の範囲は変えない
  running.value = true
  // 送る画像は入力欄から外し、送り終わるまで会話の側に見せておく。実行中も次の指示を書いて画像を足せる
  const sending = attachments.value
  attachments.value = []
  attachError.value = ''
  live.value = [{ kind: 'user', text, images: sending.map((a) => a.url) }]
  prompt.value = ''
  await scrollDown()
  try {
    const images = await Promise.all(sending.map((a) => toPayload(a.file)))
    await sendInstruction(
      props.id,
      text,
      chosen,
      async (ev) => {
        live.value.push(ev)
        await scrollDown()
      },
      images,
    )
  } catch (e) {
    live.value.push({ kind: 'done', ok: false, text: String(e.message ?? e) })
  } finally {
    running.value = false
    // 記録に追記された分を読み直す（途中経過の表示は履歴に置き換わる）
    const ended = live.value.at(-1)
    await load()
    live.value = ended?.kind === 'done' && !ended.ok ? [ended] : []
    for (const a of sending) URL.revokeObjectURL(a.url)
    emit('sent')
  }
}

// 中断: このセッションで実行中の指示を止める（ここから送ったものも、別のタブや「ここで始める」で動いているものも）。
// 止まるのは実行だけで、ここまでの変更は元に戻らない
const stopping = ref(false)
async function stop() {
  if (!window.confirm(t('conv.stopConfirm'))) return
  stopping.value = true
  try {
    await stopInstruction(props.id)
  } catch (e) {
    error.value = String(e.message ?? e)
  } finally {
    stopping.value = false
  }
}

async function scrollDown() {
  await nextTick()
  list.value?.scrollTo({ top: list.value.scrollHeight })
}

function onKey(e) {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    send()
  }
}
</script>

<template>
  <div class="conv">
    <div ref="list" class="list">
      <p v-if="error" class="chip warn">{{ error }}</p>
      <div v-if="start > 0" class="older">
        <button class="btn" :disabled="loadingOlder" @click="older">{{ t('conv.older') }}</button>
        <span class="muted small">{{ t('conv.count', { shown: total - start, total }) }}</span>
      </div>
      <template v-for="(it, i) in items" :key="start + i">
        <div v-if="it.kind === 'user'" class="msg user">
          <div class="bubble">{{ it.text }}</div>
          <div class="when muted">{{ clock(it.ts) }}</div>
        </div>
        <div v-else-if="it.kind === 'assistant'" class="msg agent">
          <div class="md" v-html="renderMarkdown(it.text)" />
        </div>
        <details v-else-if="it.kind === 'tool_use'" class="tool">
          <summary><span class="name">{{ it.name }}</span> <span class="arg">{{ it.text.split('\n')[0] }}</span></summary>
          <pre>{{ it.text }}</pre>
        </details>
        <details v-else class="tool result">
          <summary><span class="name">{{ t('conv.result') }}</span> <span class="arg">{{ it.text.split('\n')[0] }}</span></summary>
          <pre>{{ it.text }}</pre>
        </details>
      </template>

      <!-- 実行中の途中経過 -->
      <template v-for="(ev, i) in live" :key="`live${i}`">
        <div v-if="ev.kind === 'user'" class="msg user">
          <div class="bubble">{{ ev.text }}</div>
          <div v-if="ev.images?.length" class="thumbs sent"><img v-for="u in ev.images" :key="u" :src="u" alt="" /></div>
        </div>
        <div v-else-if="ev.kind === 'text'" class="msg agent"><div class="md" v-html="renderMarkdown(ev.text)" /></div>
        <div v-else-if="ev.kind === 'tool'" class="tool flat"><span class="name">{{ ev.name }}</span> <span class="arg">{{ ev.text }}</span></div>
        <PermissionAsk v-else-if="ev.kind === 'permission'" :ask="ev" />
        <p v-else-if="ev.kind === 'done' && ev.stopped" class="chip">{{ t('conv.stopped') }}</p>
        <p v-else-if="ev.kind === 'done' && !ev.ok" class="chip warn">{{ t('conv.failed', { e: ev.text ?? '' }) }}</p>
      </template>
      <PermissionAsk v-for="a in waiting" :key="a.request_id" :ask="a" />
      <p v-if="running" class="muted small running">{{ t('conv.sending') }}</p>
    </div>

    <div class="composer" @dragover.prevent @drop.prevent="onDrop">
      <p v-if="blocked" class="muted small">{{ blocked }}</p>
      <!-- 送れないあいだも書ける（実行中に次の指示やメモを書きためておく）。送るのは終わってから -->
      <template v-if="!remote">
        <textarea v-model="prompt" rows="3" :placeholder="t('conv.placeholder')" @keydown="onKey" @paste="onPaste" />
        <div v-if="attachments.length" class="thumbs">
          <span v-for="(a, i) in attachments" :key="a.url" class="thumb">
            <img :src="a.url" alt="" />
            <button class="x" :aria-label="t('conv.removeImage')" @click="removeImage(i)">✕</button>
          </span>
        </div>
        <p v-if="attachError" class="chip warn">{{ attachError }}</p>
        <div class="row">
          <select v-model="mode" :aria-label="t('conv.modeRead')">
            <option value="read">{{ t('conv.modeRead') }}</option>
            <option value="edit">{{ t('conv.modeEdit') }}</option>
            <option value="auto">{{ t('conv.modeAuto') }}</option>
          </select>
          <span class="spacer" />
          <!-- 別のタブや「ここで始める」で動いている指示も、ここから止められる -->
          <button v-if="running || following" class="btn" :disabled="stopping" @click="stop">{{ t('conv.stop') }}</button>
          <button class="btn primary" :disabled="!canSend" @click="send">{{ running ? t('conv.sending') : t('conv.send') }}</button>
        </div>
        <p class="muted small">{{ t('conv.note') }} {{ t('conv.attach', { n: MAX_IMAGES }) }}</p>
      </template>
    </div>
  </div>
</template>

<style scoped>
.conv{display:flex; flex-direction:column; height:100%; min-height:0}
.list{flex:1; min-height:0; overflow-y:auto; padding:8px 4px 12px}
.older{display:flex; align-items:center; gap:10px; margin-bottom:12px}
.small{font-size:12px}
.msg{margin:12px 0}
.msg.user{display:flex; flex-direction:column; align-items:flex-end}
.bubble{max-width:85%; background:var(--accent-faint); border-radius:16px 16px 4px 16px; padding:8px 12px; white-space:pre-wrap; word-break:break-word}
.when{font-size:11px; margin-top:2px}
/* マークダウンの返答。見出しは控えめに、コードと表は横にはみ出したらスクロール */
/* 親の pre-wrap を受け継ぐと、組み立てた HTML の改行が空行として出るので戻す（コードは pre の中で改行が残る） */
.md{line-height:1.75; word-break:break-word; white-space:normal}
.md :deep(> :first-child){margin-top:0}
.md :deep(> :last-child){margin-bottom:0}
.md :deep(p){margin:.6em 0}
.md :deep(h1),.md :deep(h2),.md :deep(h3),.md :deep(h4){font-size:15px; font-weight:600; margin:1em 0 .4em; color:var(--ink)}
.md :deep(h1){font-size:17px}
.md :deep(ul),.md :deep(ol){margin:.6em 0; padding-left:1.4em}
.md :deep(li){margin:.3em 0}
/* 項目の間が空いた箇条書きは、項目ごとに段落になる。その段落の余白は付けない */
.md :deep(li > p){margin:0}
.md :deep(code){font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace; font-size:12.5px; background:var(--hover); border-radius:4px; padding:1px 5px}
.md :deep(pre){background:var(--panel); border:1px solid var(--rule-soft); border-radius:8px; padding:10px 12px; overflow-x:auto; margin:.6em 0}
.md :deep(pre code){background:none; padding:0; font-size:12px; white-space:pre}
.md :deep(table){border-collapse:collapse; margin:.6em 0; display:block; overflow-x:auto; font-size:13px}
.md :deep(th),.md :deep(td){border:1px solid var(--rule); padding:4px 8px; text-align:left; vertical-align:top}
.md :deep(th){background:var(--panel); font-weight:500}
.md :deep(blockquote){margin:.6em 0; padding:2px 12px; border-left:3px solid var(--rule); color:var(--ink-soft)}
.md :deep(a){color:var(--accent)}
.md :deep(hr){border:0; border-top:1px solid var(--rule); margin:1em 0}
.tool{margin:4px 0; font-size:12px; color:var(--ink-soft)}
.tool summary,.tool.flat{display:flex; gap:8px; align-items:baseline; cursor:pointer; padding:3px 8px; border-radius:6px; background:var(--hover); overflow:hidden}
.tool.flat{cursor:default}
.tool .name{flex:none; font-weight:500; color:var(--ink)}
.tool .arg{min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-family:"Roboto Mono","Noto Sans Mono CJK JP",monospace}
.tool.result summary{background:none; border:1px solid var(--rule-soft)}
.tool pre{margin:4px 0 0; padding:8px; max-height:320px; overflow:auto; background:var(--panel); border-radius:6px; white-space:pre-wrap; word-break:break-word; font-size:12px}
.running{margin:8px 0}
.thumbs{display:flex; flex-wrap:wrap; gap:8px; margin-top:8px}
.thumbs.sent{justify-content:flex-end; margin-top:4px}
.thumbs img{display:block; width:72px; height:72px; object-fit:cover; border:1px solid var(--rule); border-radius:8px}
.thumb{position:relative}
.thumb .x{position:absolute; top:-6px; right:-6px; width:20px; height:20px; padding:0; border:1px solid var(--rule); border-radius:50%;
  background:var(--ground); font-size:11px; line-height:1; color:var(--ink-soft)}
.composer{border-top:1px solid var(--rule); padding:10px 0 0}
.composer > .small:first-child{margin:0 0 8px}
textarea{width:100%; resize:vertical; border:1px solid var(--outline); border-radius:8px; padding:8px 10px; font:inherit; background:var(--ground)}
textarea:focus{outline:2px solid var(--accent); border-color:transparent}
.row{display:flex; align-items:center; gap:8px; margin-top:8px}
.spacer{flex:1}
select{height:36px; border:1px solid var(--outline); border-radius:8px; padding:0 8px; background:var(--ground)}
</style>
