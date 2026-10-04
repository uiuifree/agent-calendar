import { onMounted, onUnmounted } from 'vue'

// Esc でダイアログを閉じるか。日本語入力の変換中の Esc は変換の取り消しなので閉じない
export const closesOnEscape = (e) => e.key === 'Escape' && !e.isComposing

// 開いている間だけ Esc を拾って close を呼ぶ（入力欄にフォーカスがあっても閉じる）
export function useEscape(close) {
  const onKey = (e) => {
    if (!closesOnEscape(e)) return
    e.preventDefault()
    close()
  }
  onMounted(() => window.addEventListener('keydown', onKey))
  onUnmounted(() => window.removeEventListener('keydown', onKey))
}
