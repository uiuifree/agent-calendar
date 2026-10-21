import { expect, it } from 'vitest'
import { askCount, hostErrors, jobParts, pageTitle, runningMarks } from './running.js'

const running = {
  runs: [
    { key: 'a', session_id: 's1', asks: 2 },
    { key: 'b', session_id: null, asks: 0 },
    { key: 'c', session_id: 's2', asks: 0 },
  ],
}

it('セッションごとの印と許可待ちの数', () => {
  expect(runningMarks(running)).toEqual({ s1: { asks: 2 }, s2: { asks: 0 } })
  expect(runningMarks(null)).toEqual({})
  expect(askCount(running)).toBe(2)
  expect(askCount(null)).toBe(0)
})

it('許可待ちがあればタブの題の頭に件数', () => {
  expect(pageTitle('Agent Calendar', 2)).toBe('(2) Agent Calendar')
  expect(pageTitle('Agent Calendar', 0)).toBe('Agent Calendar')
})

it('裏の処理の名前と、取り込みに失敗しているホスト', () => {
  expect(jobParts('scan')).toEqual({ kind: 'scan', machine: '' })
  expect(jobParts('scan:pi')).toEqual({ kind: 'scan', machine: 'pi' })
  expect(jobParts('remote:ai-node')).toEqual({ kind: 'remote', machine: 'ai-node' })
  expect(
    hostErrors([
      { name: 'remote:pi', error: 'refused' },
      { name: 'remote:ok', error: null },
      { name: 'scan', error: 'bad' },
    ]),
  ).toEqual({ pi: 'refused' })
  expect(hostErrors(null)).toEqual({})
})
