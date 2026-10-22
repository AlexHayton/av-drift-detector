import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import avd from '../index.js'

const tick = () => new Promise((resolve) => setImmediate(resolve))

test('generate then analyze measures the injected offset', async (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'av-drift-'))
  t.after(() => rmSync(dir, { recursive: true, force: true }))
  const output = join(dir, 'sync.mp4')

  const progress = []
  const summary = await avd.generate(
    { output, fps: '29.97', duration: 3, width: 640, height: 360, audioOffset: 0.025 },
    (p) => progress.push(p),
  )
  assert.equal(summary.frames, 90)
  assert.equal(summary.fps, '30000/1001')
  assert.equal(summary.output, output)
  await tick()
  assert.ok(progress.length > 10, `${progress.length} progress calls`)
  assert.deepEqual(progress.at(-1), { framesDone: 90, framesTotal: 90 })

  const report = await avd.analyze(output)
  assert.equal(report.capture, 'file')
  assert.equal(report.fps, '30000/1001')
  assert.equal(report.fpsSource, 'barcode')
  assert.equal(report.frames.length, 90)
  assert.ok(report.summary.measured > 80, `${report.summary.measured} measured`)
  assert.ok(Math.abs(report.summary.meanMs - 25) < 0.5, `mean ${report.summary.meanMs} ms`)
  const measured = report.frames.filter((f) => f.status === 'ok')
  assert.ok(measured.every((f) => Math.abs(f.driftMs - 25) < 1))

  // An analysis window, and a round trip through JSON.
  const window = await avd.analyze(output, { offset: 1, duration: 1 })
  assert.ok(window.frames.length >= 29 && window.frames.length <= 31, `${window.frames.length} frames`)
  assert.equal(JSON.parse(JSON.stringify(report)).summary.measured, report.summary.measured)
})

test('errors reject with a message', async () => {
  await assert.rejects(avd.analyze('/no/such/file.mp4'), /file|directory/i)
  // Bad arguments reject too, rather than throwing from an async function.
  await assert.rejects(avd.analyze('x.mp4', { capture: 'phone' }), /capture mode/)
  await assert.rejects(avd.generate({ output: 'x.mp4', fps: 'fast' }), /frame rate/)
  const stamp = { frameId: 1, source: 'paper' }
  await assert.rejects(avd.analyzeFrames({ frames: [{ time: 0, stamp }], width: 1, height: 1 }), /stamp source/)
  // Synchronous functions throw.
  assert.throws(() => avd.readFrame(new Uint8Array(10), 640, 360), /needs 230400 bytes/)
})

test('without ffmpeg, file functions say how to get it', () => {
  const script = `require(${JSON.stringify(fileURLToPath(new URL('../index.js', import.meta.url)))})
    .analyze('x.mp4').catch((e) => console.log(e.message))`
  const env = { ...process.env, AV_DRIFT_FFPROBE: '/nonexistent/ffprobe' }
  const out = execFileSync(process.execPath, ['-e', script], { env, encoding: 'utf8' })
  assert.match(out, /couldn't find `\/nonexistent\/ffprobe`: install FFmpeg 5\.1 or later/)
})

test('frames and audio in memory', async () => {
  const fps = '25'
  const renderer = new avd.PatternRenderer({ fps, width: 640, height: 360 })
  assert.equal(renderer.width, 640)
  const frames = []
  for (let n = 0; n < 75; n++) {
    const luma = renderer.render(n)
    assert.equal(luma.length, 640 * 360)
    frames.push({ time: n / 25, stamp: avd.readFrame(luma, 640, 360) })
  }
  assert.equal(frames[42].stamp.frameId, 42)
  assert.equal(frames[42].stamp.fps, '25')
  assert.equal(frames[42].stamp.source, 'barcode')

  // The QR code alone, with the barcode strip wiped.
  const luma = renderer.render(7)
  luma.fill(16, 0, 640 * 100)
  const qr = avd.readFrame(luma, 640, 360)
  assert.equal(qr.frameId, 7)
  assert.equal(qr.source, 'qr')
  assert.equal(qr.toneBaseHarmonic, 24)
  assert.equal(avd.readFrame(luma, 640, 360, null, false), null)

  const samples = avd.synthesizeAudio({ fps, delay: 0.012, count: 48000 * 3.5 })
  assert.ok(samples instanceof Float32Array)
  const report = await avd.analyzeFrames({
    frames,
    audio: { sampleRate: 48000, startTime: 0, samples },
    width: 640,
    height: 360,
  })
  assert.equal(report.capture, 'file')
  assert.ok(report.summary.measured > 60, `${report.summary.measured} measured`)
  assert.ok(Math.abs(report.summary.meanMs - 12) < 0.01, `mean ${report.summary.meanMs} ms`)
})

test('music and length options', () => {
  const withMusic = avd.synthesizeAudio({ fps: '25', count: 4800 })
  const without = avd.synthesizeAudio({ fps: '25', count: 4800, music: false })
  assert.notDeepEqual(withMusic, without)
  // The info line shows the length only when it's given.
  const plain = new avd.PatternRenderer({ fps: '25', width: 640, height: 360 }).render(0)
  const timed = new avd.PatternRenderer({ fps: '25', width: 640, height: 360, duration: 6 }).render(0)
  assert.ok(!plain.equals(timed))
  assert.throws(() => new avd.PatternRenderer({ duration: 0 }), /duration must be positive/)
})
