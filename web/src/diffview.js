// 差分を GitHub と同じ形（旧・新の行番号つきの行）にする

// 差分の 1 行の種類。inHunk は @@ の見出しより後か（その中では --- で始まる行も削除の行。
// Markdown の --- を消すと ---- になる）。見出しより前は git が付けるファイルの情報
export function lineKind(line, inHunk) {
  if (line.startsWith('diff ')) return 'meta' // 次のファイル
  if (line.startsWith('@@')) return 'hunk'
  if (!inHunk) return 'meta'
  if (line.startsWith('\\')) return 'meta' // \ No newline at end of file
  if (line.startsWith('+')) return 'add'
  if (line.startsWith('-')) return 'del'
  return 'ctx'
}

// 差分の本文 → [{ kind, old, new, text }]。old・new は旧・新の行番号（無い側は null）。
// git が付けるファイルの情報の行は省き、見出し（@@）の行は text にそのまま入れる
export function patchRows(patch) {
  const rows = []
  let [o, n, inHunk] = [0, 0, false]
  for (const line of patch.split('\n')) {
    const kind = lineKind(line, inHunk)
    if (line.startsWith('diff ')) inHunk = false
    if (kind === 'meta') continue
    if (kind === 'hunk') {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line)
      if (m) [o, n] = [Number(m[1]), Number(m[2])]
      inHunk = true
      rows.push({ kind, old: null, new: null, text: line })
      continue
    }
    if (kind === 'add') rows.push({ kind, old: null, new: n++, text: line.slice(1) })
    else if (kind === 'del') rows.push({ kind, old: o++, new: null, text: line.slice(1) })
    else rows.push({ kind, old: o++, new: n++, text: line.slice(1) })
  }
  // 末尾の改行で出来た空の行は落とす
  while (rows.length && rows.at(-1).kind === 'ctx' && rows.at(-1).text === '') rows.pop()
  return rows
}

// パスをファイル名と置き場所に分ける（一覧ではファイル名を目立たせる）
// 「変更」タブで前と後を並べて見せる画像（サーバーと同じ拡張子。SVG は入れない）
export const isImage = (path) => /\.(png|jpe?g|gif|webp)$/i.test(path)

export function splitPath(path) {
  const i = path.lastIndexOf('/')
  return i < 0 ? { dir: '', name: path } : { dir: path.slice(0, i + 1), name: path.slice(i + 1) }
}
