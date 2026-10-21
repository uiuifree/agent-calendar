// 会話の入力欄の書きかけ（指示と画像）の扱い。部品の外の預かり（セッション ID ごと）に持つ

// 預かりを捨ててよいか。空でも、送っている最中は捨てない（断られたら戻す先なので）
export const canDrop = (d) => !d.sending && !d.text && !d.attachments.length

// 送り終わったときの結果。ended は最後に届いた途中経過。
// failed = 失敗で終わった。rejected = 途中経過が何も届かずに失敗した（断られた。中断したものは除く）ので、書きかけと画像を戻す
export function sendOutcome(ended, progressed) {
  const failed = ended?.kind === 'done' && !ended.ok
  return { failed, rejected: failed && !ended.stopped && !progressed }
}

// 断られた指示を入力欄に戻す。実行中に書き足した分は後ろに残す
export const restoredText = (sent, current) => (current.trim() ? `${sent}\n\n${current}` : sent)
