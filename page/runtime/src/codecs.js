// `Codecs`: the browser's codecs (WebCodecs' VideoDecoder and VideoEncoder,
// AudioDecoder and AudioEncoder) for programs, the service `codecs` of
// /dev/gex-services (services.js). A Mac program's VideoToolbox and
// AudioToolbox (pyrus's) are its users:
// a decompression or compression session is a connection of its own (the
// bulk node where the page reserved one, else the ordinary node), which
// configures one decoder or encoder, gives it samples or frames in order,
// and hears frames or samples back in the order the codec makes them, each
// with the program's id for it; a flush is answered once everything given
// before it has come out. Capability questions (isConfigSupported) are a
// short connection's. The README's Services section, Codecs, is the
// reference for the messages.
//
// An audio session is the same, its samples float32 interleaved both ways
// (AudioData copied out at once, in the decoder's order, and closed).
//
// What a decoder makes is copied out of its VideoFrame at once (copyTo, in
// the format the program asked for: NV12, I420 or BGRA) into one buffer
// that goes to the program's ring, and the frame closed, so the decoder is
// never starved of its own frames; the decoder is fed only while what is
// buffered for the program is under a few frames.
//
// WebCodecs wants a key frame after every flush. VideoToolbox does not (a
// program waits for its frames whenever it likes, then goes on decoding),
// so the samples since the last key frame are kept, and a delta frame
// after a flush is decoded after them again, their frames not sent twice.
//
// Everything a program sends is its word: sizes are bounded, data is
// checked against its frame before it is read, and a codec's error ends
// that session's codec (the program is told), never the page.
//
// The same provider serves on the page's thread (`new Codecs(sys)`) or in a
// Worker (codecs-worker.js, `new Codecs(sys, { worker })`): WebCodecs is in
// both.

import { internals } from './internal.js';

/** What a session takes of a program. */
export const LIMITS = Object.freeze({
  // A frame's sides, and how much of a group of pictures is kept to decode
  // again after a flush.
  side: 16384, replayBytes: 64 << 20, replayChunks: 2048,
  // Frames' bytes waiting for the program before the decoder is fed more.
  buffered: 3,
  // An audio session's channels.
  channels: 32,
});

const FORMATS = new Set(['NV12', 'I420', 'BGRA', 'RGBA']);
const HARDWARE = new Set(['no-preference', 'prefer-hardware', 'prefer-software']);

const has = (name) => typeof globalThis[name] === 'function';

/** `{at, size}` of `data`, checked: the bytes, or null. */
function slice(ref, data) {
  if (!ref || typeof ref !== 'object') return null;
  const { at, size } = ref;
  if (!Number.isInteger(at) || !Number.isInteger(size) || at < 0 || size < 0 || at + size > data.length) return null;
  return data.subarray(at, at + size);
}

const codecString = (c) => (typeof c === 'string' && c.length <= 128 && /^[A-Za-z0-9.+-]+$/.test(c) ? c : null);
const side = (n) => Number.isInteger(n) && n > 0 && n <= LIMITS.side;
const pref = (h) => (HARDWARE.has(h) ? h : 'no-preference');

/** A decoder's or an encoder's configuration from a program's message, or a
 *  reason it is none. */
function decoderConfig(m, data) {
  const codec = codecString(m.codec);
  if (!codec) return { why: 'no codec' };
  const c = { codec, hardwareAcceleration: pref(m.hardware), optimizeForLatency: m.optimizeForLatency !== false };
  if (m.description) {
    const d = slice(m.description, data);
    if (!d) return { why: 'a description outside its frame' };
    c.description = d.slice();
  }
  if (side(m.width) && side(m.height)) c.codedWidth = m.width, c.codedHeight = m.height;
  return { config: c };
}

function encoderConfig(m) {
  const codec = codecString(m.codec);
  if (!codec) return { why: 'no codec' };
  if (!side(m.width) || !side(m.height)) return { why: 'no size' };
  const c = { codec, width: m.width, height: m.height, hardwareAcceleration: pref(m.hardware),
    latencyMode: m.latencyMode === 'realtime' ? 'realtime' : 'quality' };
  if (Number.isFinite(m.bitrate) && m.bitrate > 0) c.bitrate = Math.round(m.bitrate);
  if (Number.isFinite(m.framerate) && m.framerate > 0) c.framerate = m.framerate;
  if (['constant', 'variable', 'quantizer'].includes(m.bitrateMode)) c.bitrateMode = m.bitrateMode;
  if (typeof m.scalabilityMode === 'string' && /^L\dT\d$/.test(m.scalabilityMode)) c.scalabilityMode = m.scalabilityMode;
  // (Samples as a Mac's are, length-prefixed with a description, unless
  // the program asks for Annex B.)
  const format = m.format === 'annexb' ? 'annexb' : codec.startsWith('hvc1') || codec.startsWith('hev1') ? 'hevc' : 'avc';
  if (/^(avc1|avc3)\./.test(codec)) c.avc = { format: format === 'annexb' ? 'annexb' : 'avc' };
  if (/^(hvc1|hev1)\./.test(codec)) c.hevc = { format: format === 'annexb' ? 'annexb' : 'hevc' };
  return { config: c };
}

/** An audio decoder's or encoder's configuration, or a reason it is none. */
function audioConfig(m, data, encoder) {
  const codec = codecString(m.codec);
  if (!codec) return { why: 'no codec' };
  const rate = m.sampleRate, channels = m.channels;
  if (!Number.isInteger(rate) || rate < 1 || rate > 768000 || !Number.isInteger(channels) || channels < 1 || channels > LIMITS.channels) return { why: 'no rate or channels' };
  const c = { codec, sampleRate: rate, numberOfChannels: channels };
  if (!encoder) {
    if (m.description) {
      const d = slice(m.description, data);
      if (!d) return { why: 'a description outside its frame' };
      c.description = d.slice();
    }
    return { config: c };
  }
  if (Number.isFinite(m.bitrate) && m.bitrate > 0) c.bitrate = Math.round(m.bitrate);
  if (['constant', 'variable'].includes(m.bitrateMode)) c.bitrateMode = m.bitrateMode;
  if (codec === 'opus') {
    const o = {};
    // (A packet's duration, in microseconds: Opus's 2.5 to 120 ms.)
    if (Number.isInteger(m.frameDuration) && m.frameDuration >= 2500 && m.frameDuration <= 120000) o.frameDuration = m.frameDuration;
    if (Number.isInteger(m.complexity) && m.complexity >= 0 && m.complexity <= 10) o.complexity = m.complexity;
    if (['audio', 'voip', 'lowdelay'].includes(m.application)) o.application = m.application;
    c.opus = { format: 'opus', ...o };
  }
  // (AAC as raw access units, its AudioSpecificConfig the description.)
  if (codec.startsWith('mp4a.')) c.aac = { format: 'aac' };
  return { config: c };
}

/** A decoded AudioData's samples, float32 interleaved. */
function audioBytes(a) {
  const n = a.numberOfFrames, ch = a.numberOfChannels;
  const out = new Float32Array(n * ch);
  if (a.format === 'f32') a.copyTo(out, { planeIndex: 0 });
  else if (a.format === 'f32-planar') {
    const plane = new Float32Array(n);
    for (let c = 0; c < ch; c++) {
      a.copyTo(plane, { planeIndex: c });
      for (let i = 0; i < n; i++) out[i * ch + c] = plane[i];
    }
  } else a.copyTo(out, { planeIndex: 0, format: 'f32' });
  return out;
}

/** One program's audio decoding session: packets in, in order; their
 *  samples out as the decoder makes them. */
class AudioDecodeSession {
  #conn; #decoder = null; #config = null; #closed = false;

  constructor(conn) {
    this.#conn = conn;
  }

  configure(m, data) {
    const { config, why } = audioConfig(m, data, false);
    if (!config) return this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: why });
    if (!has('AudioDecoder')) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: 'no WebCodecs here' });
    AudioDecoder.isConfigSupported(config).then((r) => {
      if (this.#closed) return;
      if (!r.supported) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: `${config.codec} cannot be decoded here` });
      this.#close();
      this.#config = config;
      try {
        this.#open();
      } catch (e) {
        return this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) });
      }
      this.#conn.send({ seq: m.seq, ok: true, codec: config.codec });
    }, (e) => this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) }));
  }

  #open() {
    this.#decoder = new AudioDecoder({ output: (a) => this.#got(a), error: (e) => this.#failed(e) });
    this.#decoder.configure(this.#config);
  }

  #close() {
    try { this.#decoder?.close(); } catch {}
    this.#decoder = null;
  }

  #got(a) {
    try {
      const b = audioBytes(a);
      this.#conn.send({ op: 'audio', id: a.timestamp, frames: a.numberOfFrames, channels: a.numberOfChannels, rate: a.sampleRate, data: { at: 0, size: b.byteLength } },
        new Uint8Array(b.buffer));
    } catch (e) {
      this.#conn.send({ op: 'dropped', id: a.timestamp, error: String(e?.message ?? e) });
    } finally {
      a.close();
    }
  }

  #failed(e) {
    this.#decoder = null;
    this.#conn.send({ op: 'error', ids: [], message: String(e?.message ?? e) });
  }

  decode(m, data) {
    const bytes = slice(m.data, data);
    if (!bytes || !Number.isSafeInteger(m.id) || m.id < 0) return this.#conn.send({ op: 'dropped', id: m.id ?? -1, error: 'a packet without its data' });
    if (!this.#config) return this.#conn.send({ op: 'dropped', id: m.id, error: 'not configured' });
    try {
      if (!this.#decoder || this.#decoder.state === 'closed') this.#open();
      // (Every audio packet stands alone, as WebCodecs' audio codecs have it.)
      this.#decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: m.id, data: bytes }));
    } catch (e) {
      this.#conn.send({ op: 'dropped', id: m.id, error: String(e?.message ?? e) });
    }
  }

  flush(m) {
    const d = this.#decoder;
    if (!d || d.state !== 'configured') return this.#conn.send({ seq: m.seq, op: 'flushed', dropped: [] });
    d.flush().catch(() => {}).then(() => this.#conn.send({ seq: m.seq, op: 'flushed', dropped: [] }));
  }

  reset(m) {
    this.#close();
    try {
      if (this.#config) this.#open();
    } catch {}
    this.#conn.send({ seq: m.seq, op: 'reset', dropped: [] });
  }

  end() {
    this.#closed = true;
    this.#close();
  }
}

/** One program's audio encoding session: samples in, packets out. */
class AudioEncodeSession {
  #conn; #encoder = null; #config = null; #closed = false; #sentDescription = null;

  constructor(conn) {
    this.#conn = conn;
  }

  configure(m, data) {
    const { config, why } = audioConfig(m, data, true);
    if (!config) return this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: why });
    if (!has('AudioEncoder')) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: 'no WebCodecs here' });
    AudioEncoder.isConfigSupported(config).then((r) => {
      if (this.#closed) return;
      if (!r.supported) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: `${config.codec} cannot be encoded here` });
      try {
        try { this.#encoder?.close(); } catch {}
        this.#encoder = new AudioEncoder({ output: (c, meta) => this.#got(c, meta), error: (e) => this.#failed(e) });
        this.#encoder.configure(config);
        this.#config = config;
        this.#sentDescription = null;
        this.#conn.send({ seq: m.seq, ok: true, codec: config.codec });
      } catch (e) {
        this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) });
      }
    }, (e) => this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) }));
  }

  #got(chunk, meta) {
    const bytes = new Uint8Array(chunk.byteLength);
    chunk.copyTo(bytes);
    const m = { op: 'chunk', id: chunk.timestamp, key: true, duration: chunk.duration ?? null };
    let out = bytes;
    const d = meta?.decoderConfig?.description;
    if (d) {
      const desc = d instanceof ArrayBuffer ? new Uint8Array(d) : new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
      const same = this.#sentDescription && this.#sentDescription.length === desc.length && this.#sentDescription.every((v, i) => v === desc[i]);
      if (!same) {
        this.#sentDescription = desc.slice();
        out = new Uint8Array(bytes.length + desc.length);
        out.set(bytes);
        out.set(desc, bytes.length);
        m.description = { at: bytes.length, size: desc.length };
      }
    }
    if (meta?.decoderConfig) m.rate = meta.decoderConfig.sampleRate, m.channels = meta.decoderConfig.numberOfChannels;
    m.data = { at: 0, size: bytes.length };
    this.#conn.send(m, out);
  }

  #failed(e) {
    this.#encoder = null;
    this.#conn.send({ op: 'error', message: String(e?.message ?? e) });
  }

  encode(m, data) {
    const id = m.id;
    if (!Number.isSafeInteger(id) || id < 0) return;
    if (!this.#encoder || this.#encoder.state !== 'configured') return this.#conn.send({ op: 'dropped', id, error: 'not configured' });
    const bytes = slice(m.data, data), ch = this.#config.numberOfChannels, n = m.frames;
    if (!bytes || !Number.isInteger(n) || n < 1 || bytes.length !== n * ch * 4) return this.#conn.send({ op: 'dropped', id, error: 'samples that are not' });
    let a;
    try {
      // (Copied: the frame's data is the connection's.)
      a = new AudioData({ format: 'f32', sampleRate: this.#config.sampleRate, numberOfFrames: n, numberOfChannels: ch, timestamp: id, data: bytes.slice() });
      this.#encoder.encode(a);
    } catch (e) {
      this.#conn.send({ op: 'dropped', id, error: String(e?.message ?? e) });
    } finally {
      a?.close();
    }
  }

  flush(m) {
    if (!this.#encoder || this.#encoder.state !== 'configured') return this.#conn.send({ seq: m.seq, op: 'flushed' });
    this.#encoder.flush().catch(() => {}).then(() => this.#conn.send({ seq: m.seq, op: 'flushed' }));
  }

  end() {
    this.#closed = true;
    try { this.#encoder?.close(); } catch {}
    this.#encoder = null;
  }
}

/** A VideoFrame's colour, as code points (ISO/IEC 23091-2) a program's
 *  framework names. */
const PRIMARIES = { bt709: 1, bt470bg: 5, smpte170m: 6, bt2020: 9, smpte432: 12 };
const TRANSFER = { bt709: 1, smpte170m: 6, linear: 8, 'iec61966-2-1': 13, pq: 16, hlg: 18 };
const MATRIX = { rgb: 0, bt709: 1, bt470bg: 5, smpte170m: 6, 'bt2020-ncl': 9 };
const NAMES = (t) => Object.fromEntries(Object.entries(t).map(([k, v]) => [v, k]));
const PRIMARY_NAMES = NAMES(PRIMARIES), TRANSFER_NAMES = NAMES(TRANSFER), MATRIX_NAMES = NAMES(MATRIX);

function colourOf(cs) {
  return { primaries: PRIMARIES[cs?.primaries] ?? 2, transfer: TRANSFER[cs?.transfer] ?? 2, matrix: MATRIX[cs?.matrix] ?? 2, fullRange: cs?.fullRange ?? null };
}
function colourFrom(c) {
  if (!c || typeof c !== 'object') return undefined;
  const out = {};
  if (PRIMARY_NAMES[c.primaries]) out.primaries = PRIMARY_NAMES[c.primaries];
  if (TRANSFER_NAMES[c.transfer]) out.transfer = TRANSFER_NAMES[c.transfer];
  if (MATRIX_NAMES[c.matrix] && c.matrix) out.matrix = MATRIX_NAMES[c.matrix];
  if (typeof c.fullRange === 'boolean') out.fullRange = c.fullRange;
  return Object.keys(out).length ? out : undefined;
}

/** A decoded frame's bytes in `want` (NV12, I420, BGRA or RGBA), planes
 *  packed one after another: `{ bytes, planes: [{ at, size, stride }],
 *  width, height }`. NV12 and I420 are made of each other by moving
 *  bytes alone (exact); RGB is the browser's conversion. */
async function frameBytes(frame, want) {
  const rect = frame.visibleRect ?? { x: 0, y: 0, width: frame.codedWidth, height: frame.codedHeight };
  const w = rect.width, h = rect.height, cw = (w + 1) >> 1, ch = (h + 1) >> 1;
  if (want === 'BGRA' || want === 'RGBA') {
    const bytes = new Uint8Array(w * h * 4);
    await frame.copyTo(bytes, { format: want, layout: [{ offset: 0, stride: w * 4 }] });
    return { bytes, planes: [{ at: 0, size: bytes.length, stride: w * 4 }], width: w, height: h };
  }
  const from = frame.format;
  if (from !== 'NV12' && from !== 'I420') throw new Error(`a frame of ${from}, which is not made into ${want}`);
  const ysize = w * h;
  if (from === want) {
    const csize = want === 'NV12' ? cw * 2 * ch : cw * ch;
    const layout = want === 'NV12' ? [{ offset: 0, stride: w }, { offset: ysize, stride: cw * 2 }]
      : [{ offset: 0, stride: w }, { offset: ysize, stride: cw }, { offset: ysize + csize, stride: cw }];
    const bytes = new Uint8Array(ysize + csize * (want === 'NV12' ? 1 : 2));
    await frame.copyTo(bytes, { layout });
    const planes = layout.map((l, i) => ({ at: l.offset, size: i ? csize : ysize, stride: l.stride }));
    return { bytes, planes, width: w, height: h };
  }
  // The other 4:2:0 layout: copied as it is, then its chroma moved.
  const tmp = new Uint8Array(frame.allocationSize());
  const lay = await frame.copyTo(tmp);
  const bytes = new Uint8Array(ysize + cw * ch * 2);
  for (let y = 0; y < h; y++) bytes.set(tmp.subarray(lay[0].offset + y * lay[0].stride, lay[0].offset + y * lay[0].stride + w), y * w);
  if (want === 'NV12') {
    // I420 -> NV12: U and V interleaved.
    for (let y = 0; y < ch; y++) {
      const u = lay[1].offset + y * lay[1].stride, v = lay[2].offset + y * lay[2].stride, o = ysize + y * cw * 2;
      for (let x = 0; x < cw; x++) bytes[o + 2 * x] = tmp[u + x], bytes[o + 2 * x + 1] = tmp[v + x];
    }
    return { bytes, planes: [{ at: 0, size: ysize, stride: w }, { at: ysize, size: cw * 2 * ch, stride: cw * 2 }], width: w, height: h };
  }
  // NV12 -> I420.
  const csize = cw * ch;
  for (let y = 0; y < ch; y++) {
    const s = lay[1].offset + y * lay[1].stride, u = ysize + y * cw, v = ysize + csize + y * cw;
    for (let x = 0; x < cw; x++) bytes[u + x] = tmp[s + 2 * x], bytes[v + x] = tmp[s + 2 * x + 1];
  }
  return { bytes, planes: [{ at: 0, size: ysize, stride: w }, { at: ysize, size: csize, stride: cw }, { at: ysize + csize, size: csize, stride: cw }], width: w, height: h };
}

/** One program's decoding session. */
class DecodeSession {
  #conn; #decoder = null; #config = null; #output = { format: 'NV12' };
  #gop = []; #gopBytes = 0; #needKey = true; #suppress = new Set(); #pending = new Set();
  #queue = []; #feeding = false; #flushing = false; #closed = false;
  #frames = Promise.resolve();  // copies out of frames, in order

  constructor(conn) {
    this.#conn = conn;
    conn.addEventListener('drain', () => this.#feed());
  }

  configure(m, data) {
    const { config, why } = decoderConfig(m, data);
    if (!config) return this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: why });
    const want = FORMATS.has(m.output?.format) ? m.output.format : 'NV12';
    if (!has('VideoDecoder')) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: 'no WebCodecs here' });
    VideoDecoder.isConfigSupported(config).then((r) => {
      if (this.#closed) return;
      if (!r.supported) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: `${config.codec} cannot be decoded here` });
      this.#close();
      this.#config = config;
      this.#output = { format: want };
      this.#gop = [];
      this.#gopBytes = 0;
      this.#needKey = true;
      this.#open();
      this.#conn.send({ seq: m.seq, ok: true, codec: config.codec });
    }, (e) => this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) }));
  }

  #open() {
    this.#decoder = new VideoDecoder({
      output: (frame) => this.#got(frame),
      error: (e) => this.#failed(e),
    });
    this.#decoder.configure(this.#config);
  }

  #close() {
    try { this.#decoder?.close(); } catch {}
    this.#decoder = null;
  }

  #got(frame) {
    const id = frame.timestamp;
    if (this.#suppress.delete(id)) return frame.close();
    this.#pending.delete(id);
    const want = this.#output.format, colour = colourOf(frame.colorSpace);
    // (Copied in the order the decoder made them, each as soon as it can.)
    const copy = frameBytes(frame, want);
    this.#frames = this.#frames.then(async () => {
      try {
        const f = await copy;
        this.#conn.send({ op: 'frame', id, format: want, width: f.width, height: f.height, colour, planes: f.planes }, f.bytes);
      } catch (e) {
        this.#conn.send({ op: 'dropped', id, error: String(e?.message ?? e) });
      } finally {
        frame.close();
      }
    });
  }

  #failed(e) {
    // The decoder is closed: every frame it had is lost, and the next
    // must be a key frame (a new decoder for it).
    const ids = [...this.#pending];
    this.#pending.clear();
    this.#suppress.clear();
    this.#decoder = null;
    this.#needKey = true;
    this.#gop = [];
    this.#gopBytes = 0;
    this.#frames = this.#frames.then(() => this.#conn.send({ op: 'error', ids, message: String(e?.message ?? e) }));
  }

  decode(m, data) {
    const bytes = slice(m.data, data);
    if (!bytes || !Number.isSafeInteger(m.id) || m.id < 0) return this.#conn.send({ op: 'dropped', id: m.id ?? -1, error: 'a sample without its data' });
    this.#queue.push({ id: m.id, key: !!m.key, bytes });
    this.#feed();
  }

  /** Give the decoder what is queued, while the program keeps up. */
  #feed() {
    if (this.#feeding) return;
    this.#feeding = true;
    try {
      // (Nothing more is given while a flush is under way: what follows it
      // may need the decoder made again.)
      while (this.#queue.length && !this.#flushing) {
        const head = this.#queue[0];
        if (head.flush) {
          if (this.#decoder?.state === 'configured') {
            this.#queue.shift();
            const d = this.#decoder, seq = head.seq;
            this.#flushing = true;
            this.#needKey = true;
            d.flush().catch(() => {}).then(() => {
              this.#flushed(seq, d);
              this.#flushing = false;
              this.#feed();
            });
            continue;
          }
          this.#queue.shift();
          this.#flushed(head.seq, null);
          continue;
        }
        if (this.#conn.bufferedAmount > LIMITS.buffered * this.#frameSize()) break;
        this.#queue.shift();
        this.#one(head);
      }
    } finally {
      this.#feeding = false;
    }
  }

  #frameSize() {
    const w = this.#config?.codedWidth ?? 1920, h = this.#config?.codedHeight ?? 1080;
    return w * h * (this.#output.format === 'NV12' || this.#output.format === 'I420' ? 1.5 : 4);
  }

  #one({ id, key, bytes }) {
    if (!this.#config) return this.#conn.send({ op: 'dropped', id, error: 'not configured' });
    if (!this.#decoder || this.#decoder.state === 'closed') {
      if (!key) return this.#conn.send({ op: 'dropped', id, error: 'the decoder failed: a key frame is wanted' });
      this.#open();
    }
    if (key) {
      this.#gop = [];
      this.#gopBytes = 0;
    } else if (this.#needKey) {
      // After a flush: the group so far again, its frames not sent twice.
      if (!this.#gop.length || !this.#gop[0].key) return this.#conn.send({ op: 'dropped', id, error: 'a delta frame with no key frame before it' });
      this.#close();
      this.#open();
      for (const c of this.#gop) {
        this.#suppress.add(c.id);
        this.#decoder.decode(new EncodedVideoChunk({ type: c.key ? 'key' : 'delta', timestamp: c.id, data: c.bytes }));
      }
    }
    this.#needKey = false;
    if (this.#gopBytes + bytes.length <= LIMITS.replayBytes && this.#gop.length < LIMITS.replayChunks) {
      this.#gop.push({ id, key, bytes });
      this.#gopBytes += bytes.length;
    }
    this.#pending.add(id);
    try {
      this.#decoder.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: id, data: bytes }));
    } catch (e) {
      this.#pending.delete(id);
      this.#conn.send({ op: 'dropped', id, error: String(e?.message ?? e) });
    }
  }

  flush(m) {
    this.#queue.push({ flush: true, seq: m.seq });
    this.#feed();
  }

  #flushed(seq, decoder) {
    // Every frame of it copied out and sent first; then the samples that
    // made none are said.
    this.#frames = this.#frames.then(() => {
      const none = decoder === this.#decoder ? [...this.#pending] : [];
      for (const id of none) this.#pending.delete(id);
      this.#conn.send({ seq, op: 'flushed', dropped: none });
    });
  }

  reset(m) {
    const dropped = [...this.#pending, ...this.#queue.filter((q) => !q.flush).map((q) => q.id)];
    for (const q of this.#queue) if (q.flush) this.#conn.send({ seq: q.seq, op: 'flushed', dropped: [] });
    this.#queue = [];
    this.#pending.clear();
    this.#suppress.clear();
    this.#close();
    this.#needKey = true;
    this.#gop = [];
    this.#gopBytes = 0;
    this.#frames = this.#frames.then(() => this.#conn.send({ seq: m.seq, op: 'reset', dropped }));
  }

  end() {
    this.#closed = true;
    this.#queue = [];
    this.#close();
  }
}

/** One program's encoding session. */
class EncodeSession {
  #conn; #encoder = null; #config = null; #chunks = Promise.resolve(); #closed = false; #sentDescription = null;

  constructor(conn) {
    this.#conn = conn;
  }

  configure(m) {
    const { config, why } = encoderConfig(m);
    if (!config) return this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: why });
    if (!has('VideoEncoder')) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: 'no WebCodecs here' });
    VideoEncoder.isConfigSupported(config).then((r) => {
      if (this.#closed) return;
      if (!r.supported) return this.#conn.send({ seq: m.seq, ok: false, error: 'unsupported', message: `${config.codec} cannot be encoded here` });
      try {
        // (Configured again, a running encoder keeps what it has: a new
        // bitrate takes effect from the next frame.)
        if (!this.#encoder || this.#encoder.state === 'closed') {
          this.#encoder = new VideoEncoder({ output: (c, meta) => this.#got(c, meta), error: (e) => this.#failed(e) });
        }
        this.#encoder.configure(config);
        this.#config = config;
        this.#conn.send({ seq: m.seq, ok: true, codec: config.codec });
      } catch (e) {
        this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) });
      }
    }, (e) => this.#conn.send({ seq: m.seq, ok: false, error: 'invalid', message: String(e?.message ?? e) }));
  }

  #got(chunk, meta) {
    const bytes = new Uint8Array(chunk.byteLength);
    chunk.copyTo(bytes);
    const m = { op: 'chunk', id: chunk.timestamp, key: chunk.type === 'key', duration: chunk.duration ?? null };
    let out = bytes;
    const d = meta?.decoderConfig?.description;
    if (d) {
      const desc = d instanceof ArrayBuffer ? new Uint8Array(d) : new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
      const same = this.#sentDescription && this.#sentDescription.length === desc.length && this.#sentDescription.every((v, i) => v === desc[i]);
      if (!same) {
        this.#sentDescription = desc.slice();
        out = new Uint8Array(bytes.length + desc.length);
        out.set(bytes);
        out.set(desc, bytes.length);
        m.description = { at: bytes.length, size: desc.length };
        if (meta.decoderConfig.colorSpace) m.colour = colourOf(meta.decoderConfig.colorSpace);
      }
    }
    m.data = { at: 0, size: bytes.length };
    this.#conn.send(m, out);
  }

  #failed(e) {
    this.#encoder = null;
    this.#conn.send({ op: 'error', message: String(e?.message ?? e) });
  }

  encode(m, data) {
    const id = m.id;
    if (!Number.isSafeInteger(id) || id < 0) return;
    if (!this.#encoder || this.#encoder.state !== 'configured') return this.#conn.send({ op: 'dropped', id, error: 'not configured' });
    const format = ['NV12', 'I420', 'BGRA', 'BGRX', 'RGBA', 'RGBX'].includes(m.format) ? m.format : null;
    const planes = Array.isArray(m.planes) ? m.planes.map((p) => ({ bytes: slice(p, data), stride: p?.stride })) : [];
    if (!format || !side(m.width) || !side(m.height) || !planes.length || planes.some((p) => !p.bytes || !Number.isInteger(p.stride) || p.stride <= 0))
      return this.#conn.send({ op: 'dropped', id, error: 'a frame that is not one' });
    // One buffer, the planes where their references say (they are one
    // frame's data already).
    const first = m.planes[0].at, last = Math.max(...m.planes.map((p) => p.at + p.size));
    const layout = m.planes.map((p) => ({ offset: p.at - first, stride: p.stride }));
    let frame;
    try {
      frame = new VideoFrame(data.subarray(first, last), {
        format, codedWidth: m.width, codedHeight: m.height, timestamp: id, layout,
        ...(Number.isFinite(m.duration) && m.duration > 0 ? { duration: Math.round(m.duration) } : {}),
        ...(colourFrom(m.colour) ? { colorSpace: colourFrom(m.colour) } : {}),
      });
    } catch (e) {
      return this.#conn.send({ op: 'dropped', id, error: String(e?.message ?? e) });
    }
    try {
      this.#encoder.encode(frame, { keyFrame: !!m.key });
    } catch (e) {
      this.#conn.send({ op: 'dropped', id, error: String(e?.message ?? e) });
    } finally {
      frame.close();
    }
  }

  flush(m) {
    if (!this.#encoder || this.#encoder.state !== 'configured') return this.#conn.send({ seq: m.seq, op: 'flushed' });
    this.#encoder.flush().catch(() => {}).then(() => this.#conn.send({ seq: m.seq, op: 'flushed' }));
  }

  end() {
    this.#closed = true;
    try { this.#encoder?.close(); } catch {}
    this.#encoder = null;
  }
}

/** Whether each configuration of `configs` is supported: `[{ supported }]`. */
async function query(configs, data) {
  const out = [];
  for (const q of Array.isArray(configs) ? configs.slice(0, 64) : []) {
    try {
      if (q?.kind === 'audio-decoder' || q?.kind === 'audio-encoder') {
        const enc = q.kind === 'audio-encoder', { config } = audioConfig(q, data, enc);
        const api = enc ? 'AudioEncoder' : 'AudioDecoder';
        out.push({ supported: !!config && has(api) && !!(await globalThis[api].isConfigSupported(config)).supported });
      } else if (q?.kind === 'encoder') {
        const { config } = encoderConfig(q);
        out.push({ supported: !!config && has('VideoEncoder') && !!(await VideoEncoder.isConfigSupported(config)).supported });
      } else if (q?.kind === 'image') {
        const type = typeof q.type === 'string' && /^image\/[a-z0-9.+-]+$/.test(q.type) ? q.type : null;
        out.push({ supported: !!type && has('ImageDecoder') && !!(await ImageDecoder.isTypeSupported(type)) });
      } else {
        const { config } = decoderConfig(q ?? {}, data);
        out.push({ supported: !!config && has('VideoDecoder') && !!(await VideoDecoder.isConfigSupported(config)).supported });
      }
    } catch {
      out.push({ supported: false });
    }
  }
  return out;
}

/** An image of a file (`ImageDecoder`): `[answer, pixels]`, the pixels
 *  RGBA, 8 bits a channel, alpha last and not premultiplied, in sRGB. */
async function decodeImage(m, data) {
  const bytes = slice(m.data, data);
  const type = typeof m.type === 'string' && /^image\/[a-z0-9.+-]{1,64}$/.test(m.type) ? m.type : null;
  const index = Number.isInteger(m.index) && m.index >= 0 ? m.index : 0;
  if (!bytes || !type) return [{ ok: false, error: 'invalid' }, null];
  if (!has('ImageDecoder') || !(await ImageDecoder.isTypeSupported(type).catch(() => false))) return [{ ok: false, error: 'unsupported' }, null];
  let dec = null, image = null;
  try {
    dec = new ImageDecoder({ data: bytes, type, premultiplyAlpha: 'none' });
    await dec.tracks.ready;
    await dec.completed;
    const frames = dec.tracks.selectedTrack?.frameCount ?? 1;
    if (index >= frames) return [{ ok: false, error: 'no such image', frames }, null];
    ({ image } = await dec.decode({ frameIndex: index }));
    const rect = image.visibleRect ?? { width: image.codedWidth, height: image.codedHeight };
    const w = rect.width, h = rect.height;
    if (!side(w) || !side(h)) return [{ ok: false, error: 'too large' }, null];
    const alpha = /A/.test(image.format ?? '');
    if (m.pixels === false) return [{ ok: true, width: w, height: h, alpha, frames }, null];
    const out = new Uint8Array(w * h * 4);
    await image.copyTo(out, { format: 'RGBA', layout: [{ offset: 0, stride: w * 4 }] });
    return [{ ok: true, width: w, height: h, alpha, frames, data: { at: 0, size: out.length } }, out];
  } catch (e) {
    return [{ ok: false, error: String(e?.message ?? e) }, null];
  } finally {
    try { image?.close(); } catch {}
    try { dec?.close(); } catch {}
  }
}

/** The `codecs` service's provider: for `sys.services.provide`, or
 *  services-worker.js's `serveServices` in a Worker. */
export function codecsProvider() {
  return {
    version: 1,
    open(conn) {
      let session = null;
      conn.addEventListener('message', ({ message: m, data }) => {
        if (m.op === 'close') {
          // (The program's framework is done with it: VideoToolbox's
          // invalidate. The end is said, and its reader lets go.)
          session?.end();
          session = null;
          conn.close();
          return;
        }
        if (m.op === 'query') {
          query(m.configs, data).then((results) => conn.send({ seq: m.seq, results }));
          return;
        }
        if (m.op === 'image') {
          decodeImage(m, data).then(([answer, pixels]) => conn.send({ seq: m.seq, ...answer }, pixels));
          return;
        }
        if (m.op === 'configure') {
          const kind = { encoder: EncodeSession, 'audio-decoder': AudioDecodeSession, 'audio-encoder': AudioEncodeSession }[m.kind] ?? DecodeSession;
          if (session && !(session instanceof kind)) return conn.send({ seq: m.seq, ok: false, error: 'invalid', message: 'a session is a decoder or an encoder' });
          session ??= new kind(conn);
          return session.configure(m, data);
        }
        if (!session) return m.seq != null && conn.send({ seq: m.seq, ok: false, error: 'invalid', message: 'not configured' });
        const decoder = session instanceof DecodeSession || session instanceof AudioDecodeSession;
        if (m.op === 'decode' && decoder) session.decode(m, data);
        else if (m.op === 'encode' && !decoder) session.encode(m, data);
        else if (m.op === 'flush') session.flush(m);
        else if (m.op === 'reset' && decoder) session.reset(m);
      });
      conn.addEventListener('close', () => session?.end());
    },
  };
}

/**
 * The page offers programs its browser's codecs. One per system; made any
 * time. With `worker` (a Worker, or a MessagePort to one, whose script
 * runs codecs-worker.js's `serveCodecs`), sessions are served there and
 * the page's thread is out of their path.
 */
export class Codecs {
  constructor(sys, { worker = null } = {}) {
    internals(sys, 'Codecs');
    this.stop = sys.services.provide('codecs', worker ? { version: 1, worker } : codecsProvider());
  }

  /** Whether the browser has WebCodecs at all (a program then finds every
   *  codec missing, as a Mac with no decoder for it). */
  static get available() {
    return has('VideoDecoder') && has('VideoEncoder');
  }
}
