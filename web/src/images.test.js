import { expect, it } from 'vitest'
import { MAX_IMAGES, MAX_IMAGE_BYTES, pickImages, toBase64, toPayload } from './images.js'

const file = (type, size = 10) => ({ type, size })

it('付けられる画像だけ選び、理由を返す', () => {
  const png = file('image/png')
  expect(pickImages([png, file('text/plain')], 0)).toEqual({ accepted: [png], error: null })
  expect(pickImages([file('image/svg+xml')], 0)).toEqual({ accepted: [], error: 'conv.imageType' })
  expect(pickImages([file('image/png', MAX_IMAGE_BYTES + 1)], 0).error).toBe('conv.imageSize')
  const r = pickImages([png, png, png], MAX_IMAGES - 2)
  expect(r.accepted.length).toBe(2)
  expect(r.error).toBe('conv.imageCount')
})

it('base64 にする（区切りをまたぐ大きさでも）', async () => {
  expect(toBase64(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBe('iVBORw==')
  const big = new Uint8Array(0x8000 + 3).fill(65)
  expect(atob(toBase64(big)).length).toBe(big.length)
  const blob = new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' })
  expect(await toPayload(blob)).toEqual({ media_type: 'image/png', data: 'AQID' })
})
