import { ref, watch } from 'vue'

// 画面の状態のうち、次に開いたときも残したいものはブラウザに覚える（保存できなければ既定に戻る）。
// valid で形を確かめ、合わないもの（古い書式など）は既定にする
export function remembered(key, fallback, valid = () => true) {
  let v = fallback
  try {
    const raw = localStorage.getItem(key)
    if (raw != null) {
      const parsed = JSON.parse(raw)
      if (valid(parsed)) v = parsed
    }
  } catch {
    // 読めなければ既定
  }
  const r = ref(v)
  watch(
    r,
    (x) => {
      try {
        localStorage.setItem(key, JSON.stringify(x))
      } catch {
        // 保存できなくても画面は切り替わる
      }
    },
    { deep: true },
  )
  return r
}

// 許可の範囲（読むだけ / ファイルの編集まで / 自動判定）。「ここで始める」と続きの指示で共通。
// 既定は自動判定。選び直したらそれを次からの初期値にする
export const MODES = ['read', 'edit', 'auto']
export const rememberedMode = () => remembered('agent-calendar.mode', 'auto', (v) => MODES.includes(v))
