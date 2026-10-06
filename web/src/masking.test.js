import { expect, it } from 'vitest'
import { setLang } from './i18n.js'
import { fakePatch, fakePath, fakeText, maskData, repoName, unmask } from './masking.js'

it('リポジトリ名は同じ元の値なら同じ作り物、別の値なら別の作り物', () => {
  expect(repoName('flow-rust')).toBe(repoName('flow-rust'))
  expect(repoName('flow-rust')).not.toBe(repoName('obsidian'))
})

it('週のデータ: パス・題を置き換え、数字・状態・ID は残す', () => {
  const week = {
    sessions: [{ id: 's1', repo: '/home/u/secret-client', title: '顧客Aの請求バグ', status: 'wip', usd: [[1, 2.5]], machine: 'ai-node' }],
    repos: { '/home/u/secret-client': { name: 'secret-client', slot: 1, count: 3 } },
    machines: [{ id: '', label: 'WSL' }, { id: 'ai-node', label: 'ai-node' }],
  }
  const m = maskData(week)
  const s = m.sessions[0]
  expect(s.repo).toMatch(/^\/home\/me\/projects\//)
  expect(s.repo).not.toContain('secret')
  expect(s.title).not.toContain('顧客')
  expect([s.id, s.status, s.usd, s.machine]).toEqual(['s1', 'wip', [[1, 2.5]], 'ai-node'])
  // リポジトリの表はキーも置き換え、セッションの repo と同じ作り物になる
  expect(Object.keys(m.repos)).toEqual([s.repo])
  expect(m.repos[s.repo]).toMatchObject({ slot: 1, count: 3 })
  expect(m.machines.map((x) => x.label)).toEqual(['WSL', 'server'])
})

it('文章は行の頭の印と長さを保ち、コードの囲みも残す', () => {
  const src = '# 見出し\n- 顧客Aに連絡する\n1. 請求を直す\n\n```\nsecret_key = 1\n```'
  const out = fakeText(src).split('\n')
  expect(out[0].startsWith('# ')).toBe(true)
  expect(out[1].startsWith('- ')).toBe(true)
  expect(out[2].startsWith('1. ')).toBe(true)
  expect(out[3]).toBe('')
  expect(out[4]).toBe('```')
  expect(out[5]).not.toContain('secret')
  expect(fakeText(src)).not.toMatch(/顧客|請求/)
  // 作り物は画面の言語で作る（元が日本語でも、英語の画面なら英語）
  setLang('en')
  expect(fakeText('顧客Aの請求バグ')).toMatch(/^[A-Z][a-z]/)
  // 文の区切りのあとは大文字
  expect(fakeText('x'.repeat(200))).not.toMatch(/\. [a-z]/)
  setLang('ja')
  expect(fakeText('Fix the billing bug for ACME')).toMatch(/[ぁ-んァ-ヶ一-龠]/)
  // 助詞から始めない
  for (const s of ['あ', 'いい', '設定の項目', '画面を直す']) expect(fakeText(s)).not.toMatch(/^[をにがのはでと、。]/)
})

it('差分は行の種類を保つ', () => {
  const out = fakePatch('diff --git a/secret.rs b/secret.rs\n@@ -1,2 +1,2 @@ fn x\n-old_secret\n+new_secret\n same').split('\n')
  expect(out[0]).toBe('diff --git a/src/app b/src/app')
  expect(out[1]).toBe('@@ -1,2 +1,2 @@')
  expect(out[2][0]).toBe('-')
  expect(out[3][0]).toBe('+')
  expect(out.join('\n')).not.toContain('secret')
})

it('ツールの名前・GitHub のリンク・ファイルのパス', () => {
  const m = maskData({
    items: [{ kind: 'tool_use', name: 'Bash', text: 'cat secret.txt' }],
    github: { repo: 'https://github.com/my-org/secret', branch: 'https://github.com/my-org/secret/tree/feat/x', compare: null },
    files: [{ path: 'src/secret/billing.rs', added: 3 }],
    owners: ['my-org'],
  })
  expect(m.items[0].name).toBe('Bash')
  expect(m.items[0].text).not.toContain('secret')
  expect(m.github.repo).not.toContain('my-org')
  expect(m.github.repo).toBe(`https://github.com/${m.owners[0]}/${m.github.repo.split('/').pop()}`)
  expect(m.github.branch).toMatch(/\/tree\/feature\/example$/)
  expect(m.github.compare).toBe(null)
  // 「ここで始める」で選ぶブランチ: main はそのまま、ほかは作り物。origin/ は残す
  const b = maskData({ default: 'main', branches: ['main', 'client-acme/billing', 'origin/main', 'origin/client-acme/billing'] })
  expect(b.default).toBe('main')
  expect(b.branches[0]).toBe('main')
  expect(b.branches[1]).toMatch(/^feature\//)
  expect(b.branches[2]).toBe('origin/main')
  expect(b.branches[3]).toMatch(/^origin\/feature\//)
  expect(b.branches[3]).not.toContain('acme')
  expect(maskData({ default: null }).default).toBe(null)
  expect(maskData({ checked_out: '/home/me/Secret Client' }).checked_out).not.toMatch(/Secret/)
  expect(m.files[0].path).toMatch(/\.rs$/)
  expect(m.files[0].path).not.toContain('billing')
  expect(m.files[0].added).toBe(3)
  expect(m.owners[0]).not.toBe('my-org')
  // 組織も、別の元の名前なら別の作り物（タブが重ならない）
  expect(maskData({ owners: ['a-org', 'b-org'] }).owners[0]).not.toBe(maskData({ owners: ['b-org'] }).owners[0])
})

it('作り物から本物に戻せる（差分やセッションの一覧を取りに行くとき）。名前が尽きたら番号', () => {
  const m = maskData({ files: [{ path: 'src/a/x.rs' }, { path: 'src/b/y.rs' }], local: '/home/u/real-repo' })
  expect(unmask(m.files[0].path)).toBe('src/a/x.rs')
  expect(unmask(m.files[1].path)).toBe('src/b/y.rs')
  expect(m.files[0].path).not.toBe(m.files[1].path)
  expect(unmask(m.local)).toBe('/home/u/real-repo')
  expect(unmask('not-masked')).toBe('not-masked')
  const names = Array.from({ length: 60 }, (_, i) => repoName(`many-${i}`))
  expect(new Set(names).size).toBe(60)
  expect(names.some((n) => /^project-\d+$/.test(n))).toBe(true)
})

it('レビューで見つかった漏れ: 案件・設定・大文字の名前・差分の本文・同じ名前のパス・再開のコマンド', () => {
  const stats = maskData({ projects: [{ project: '顧客A案件', repos: [{ name: 'SecretClient', repo: '/x/SecretClient' }] }, { project: '' }], known_projects: ['顧客A案件'] })
  expect(stats.projects[0].project).not.toContain('顧客')
  expect(stats.known_projects[0]).toBe(stats.projects[0].project)
  expect(stats.projects[1].project).toBe('') // 未分類は空のまま
  expect(stats.projects[0].repos[0].name).not.toBe('SecretClient')
  const settings = maskData({ settings: { github_owners: ['secret-org'], repo_roots: ['/home/u/secret'] } })
  expect(settings.settings.github_owners[0]).not.toBe('secret-org')
  expect(settings.settings.repo_roots[0]).toBe('/home/me/projects')
  // ツールの呼び出しの name だけは残す
  const items = maskData({ items: [{ kind: 'tool_use', name: 'Bash', text: 'x' }, { kind: 'tool', name: 'Edit' }] })
  expect(items.items.map((i) => i.name)).toEqual(['Bash', 'Edit'])
  // 見出しの中の --- / +++ で始まる行も置き換える
  const patch = fakePatch('--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- secret-client\n+++ secret-client\n\\ No newline at end of file')
  expect(patch).not.toContain('secret')
  expect(patch.split('\n')[3][0]).toBe('-')
  expect(patch.split('\n')[4][0]).toBe('+')
  // 末尾の名前が同じ別のパスは別の作り物。逆引きもそれぞれに戻る
  const a = fakePath('/home/a/service')
  const b = fakePath('/home/b/service')
  expect(a).not.toBe(b)
  expect([unmask(a), unmask(b)]).toEqual(['/home/a/service', '/home/b/service'])
  // 空白を含むパス（引用符つき）も全体を置き換える
  const cmd = maskData({ resume_command: "cd '/home/me/Secret Client' && claude --resume abc" }).resume_command
  expect(cmd).not.toMatch(/Secret|Client/)
  expect(cmd).toMatch(/^cd \/home\/me\/projects\/\S+ && claude --resume abc$/)
  expect(unmask(cmd.split(' ')[1])).toBe('/home/me/Secret Client')
})
