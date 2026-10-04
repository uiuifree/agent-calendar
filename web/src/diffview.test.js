import { expect, it } from 'vitest'
import { lineKind, patchRows, splitPath } from './diffview.js'

it('差分の行を分ける（見出しより前はファイルの情報）', () => {
  expect(lineKind('diff --git a/a b/a', true)).toBe('meta')
  expect(lineKind('+++ b/a.txt', false)).toBe('meta')
  expect(lineKind('--- a/a.txt', false)).toBe('meta')
  expect(lineKind('index 1..2 100644', false)).toBe('meta')
  expect(lineKind('@@ -1,2 +1,3 @@', false)).toBe('hunk')
  expect(lineKind('\\ No newline at end of file', true)).toBe('meta')
  expect(lineKind('+three', true)).toBe('add')
  expect(lineKind('-two', true)).toBe('del')
  expect(lineKind(' one', true)).toBe('ctx')
  // 見出しの中なら --- や +++ で始まっても削除・追加の行
  expect(lineKind('----', true)).toBe('del')
  expect(lineKind('+++x', true)).toBe('add')
})

it('旧・新の行番号を付ける', () => {
  const patch = 'diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -3,3 +3,4 @@ fn x\n one\n-two\n+2\n+three\n four\n@@ -10 +11 @@\n-x\n+y\n'
  const rows = patchRows(patch)
  expect(rows.map((r) => [r.kind, r.old, r.new, r.text])).toEqual([
    ['hunk', null, null, '@@ -3,3 +3,4 @@ fn x'],
    ['ctx', 3, 3, 'one'],
    ['del', 4, null, 'two'],
    ['add', null, 4, '2'],
    ['add', null, 5, 'three'],
    ['ctx', 5, 6, 'four'],
    ['hunk', null, null, '@@ -10 +11 @@'],
    ['del', 10, null, 'x'],
    ['add', null, 11, 'y'],
  ])
  expect(patchRows('')).toEqual([])
  // Markdown の --- を消した行（----）も削除の行として数え、後ろの行番号がずれない
  const md = patchRows('--- a/x.md\n+++ b/x.md\n@@ -1,3 +1,2 @@\n a\n----\n b\n')
  expect(md.map((r) => [r.kind, r.old, r.new, r.text])).toEqual([
    ['hunk', null, null, '@@ -1,3 +1,2 @@'],
    ['ctx', 1, 1, 'a'],
    ['del', 2, null, '---'],
    ['ctx', 3, 2, 'b'],
  ])
})

it('パスを分ける', () => {
  expect(splitPath('src/a/b.rs')).toEqual({ dir: 'src/a/', name: 'b.rs' })
  expect(splitPath('README.md')).toEqual({ dir: '', name: 'README.md' })
})
