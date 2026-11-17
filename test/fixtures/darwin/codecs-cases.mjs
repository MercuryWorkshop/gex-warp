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
  // (`differs`: where this Mac's reference is not a Mac's behaviour to copy,
  // by translated architecture: under Rosetta its HEVC encoder, a software
  // one, marks every sample NotSync, key frames too.)
  { name: 'encode-hevc', prog: 'vtencode', args: ['hevc', '420v', 'realtime'],
    differs: { x86: "this Mac's HEVC encoder under Rosetta marks its key frames NotSync; ours says them (an Intel Mac's own encoder was not to hand)" } },
  // ImageIO's HEIC, AVIF and WebP, the host's ImageDecoder's (`files`: every
  // argument a file of test/media/images).
  { name: 'image-webp', prog: 'iiodecode', files: true, args: ['lossless.webp', 'animated.webp'] },
  // (Lossy WebP's colour is converted by each decoder its own way, and
  // the two are far apart: said, not judged.)
  { name: 'image-webp-lossy', prog: 'iiodecode', files: true, args: ['lossy.webp'], measure: { planes: [['RGBA', 24576, 255, 0]] } },
  { name: 'image-avif', prog: 'iiodecode', files: true, args: ['still.avif'], measure: { planes: [['RGBA', 24576, 3, 0.95]] } },
  // (A browser without HEIC: the image is known, and not made, as on a Mac
  // without the decoder; `unless` is the type, `absent` what is said then.)
  { name: 'image-heic', prog: 'iiodecode', files: true, args: ['still.heic'], unless: 'image/heic',
    absent: 'still.heic: public.heic, 1 image, status 0\n  0: properties -1x-1 alpha -1, no image\n' },
];
export const CLIPS = ['base.h264', 'base.pts', 'bframes.h264', 'bframes.pts', 'main.hevc', 'main.pts'];
export const IMAGES = ['lossless.webp', 'lossy.webp', 'animated.webp', 'still.avif', 'still.heic'];
/** A case's arguments, its files found in `dir`. */
export const argsOf = (c, dir) => (c.files ? c.args.map((a) => `${dir}/${a}`) : c.prog === 'vtdecode' ? [`${dir}/${c.args[0]}`, ...c.args.slice(1)] : c.args);
