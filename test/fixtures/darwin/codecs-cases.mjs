// darwin/codecs-page's cases: pyrus's test/media programs, each run on the
// Mac that makes the images (its VideoToolbox, the reference) and in a Mac
// root on the page (pyrus's VideoToolbox over the page's `codecs` service).
// A case is its program's arguments; `order` marks one whose frames may
// come in another order (asynchronous decoding: the browser's decoder
// gives them in presentation order, a Mac's in decode order), and
// `measure` one whose pixels are converted by each side's own arithmetic
// (another range, RGB), whose bytes are measured against the Mac's rather
// than compared: within `within` of them everywhere, and within 1 of them
// in at least `most` of them; a measured case's `planes` are the sizes of
// each frame's planes, said apart (a Mac makes 420f's chroma its own way,
// which is said and not judged).
export const CASES = [
  { name: 'base-sync', prog: 'vtdecode', args: ['base.h264', 'sync', 'none'] },
  { name: 'base-y420', prog: 'vtdecode', args: ['base.h264', 'sync', 'y420'] },
  { name: 'base-420f', prog: 'vtdecode', args: ['base.h264', 'sync', '420f'], measure: { planes: [['luma', 57600, 1, 1], ['chroma', 28800, 255, 0]] } },
  { name: 'base-bgra', prog: 'vtdecode', args: ['base.h264', 'sync', 'BGRA'], measure: { planes: [['BGRA', 230400, 3, 0.97]] } },
  { name: 'base-async', prog: 'vtdecode', args: ['base.h264', 'async', 'none'], order: true },
  { name: 'bframes-sync', prog: 'vtdecode', args: ['bframes.h264', 'sync', 'none'] },
  { name: 'bframes-wait', prog: 'vtdecode', args: ['bframes.h264', 'wait', 'none'] },
  { name: 'bframes-async', prog: 'vtdecode', args: ['bframes.h264', 'async', 'none'], order: true },
  { name: 'bframes-temporal', prog: 'vtdecode', args: ['bframes.h264', 'temporal', 'none'], order: true },
  { name: 'hevc-sync', prog: 'vtdecode', args: ['main.hevc', 'sync', 'none'] },
  { name: 'hevc-async', prog: 'vtdecode', args: ['main.hevc', 'async', 'none'], order: true },
  { name: 'encode-h264', prog: 'vtencode', args: ['h264', '420v', 'realtime'] },
  { name: 'encode-h264-bgra', prog: 'vtencode', args: ['h264', 'BGRA', 'quality'] },
  { name: 'encode-h264-y420', prog: 'vtencode', args: ['h264', 'y420', 'realtime'] },
  { name: 'encode-hevc', prog: 'vtencode', args: ['hevc', '420v', 'realtime'] },
];
export const CLIPS = ['base.h264', 'base.pts', 'bframes.h264', 'bframes.pts', 'main.hevc', 'main.pts'];
