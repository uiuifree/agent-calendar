// 指示に付ける画像（貼り付けたスクリーンショットなど）。上限はサーバー（send.rs）と同じ
export const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp']
export const MAX_IMAGES = 5
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024

// 貼り付け・ドロップされたファイルのうち、付けられるものを選ぶ。
// 戻り値の error は、付けられなかったものがあったときの理由（i18n のキー）
export function pickImages(files, already) {
  const accepted = []
  let error = null
  for (const f of files) {
    if (!IMAGE_TYPES.includes(f.type)) {
      if (f.type.startsWith('image/')) error = 'conv.imageType'
      continue
    }
    if (f.size > MAX_IMAGE_BYTES) {
      error = 'conv.imageSize'
      continue
    }
    if (already + accepted.length >= MAX_IMAGES) {
      error = 'conv.imageCount'
      break
    }
    accepted.push(f)
  }
  return { accepted, error }
}

// バイト列 → base64（大きな画像でも引数の数の上限を超えないように区切って変換する）
export function toBase64(bytes) {
  let s = ''
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

// サーバーに送る形
export async function toPayload(file) {
  return { media_type: file.type, data: toBase64(new Uint8Array(await file.arrayBuffer())) }
}
