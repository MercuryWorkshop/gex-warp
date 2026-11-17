// darwin/audio-codecs-page's cases: pyrus's test/media/atcodec, run on the
// Mac that makes the images (its AudioToolbox, the reference) and in a Mac
// root on the page (pyrus's AudioToolbox over the page's `codecs` service).
// A case is the program's arguments; a `file` case's first argument is one
// of the sounds (pyrus's test/media/sounds, made by its sounds.sh). `codec`
// is the WebCodecs codec the case needs of the browser, and `encode`
// whether it needs it to encode. A `make` case decodes packets this Mac's
// encoder made (atcodec encode, kept in `packets` when the images are
// made, beside the Mac's own output); `differs` says why a case is not what a
// Mac does here (what the browser has not got), which is said, not judged.
const LC_ONLY = "Chrome's AAC encoder on a Mac takes 44.1 and 48 kHz only, and makes AAC-LC when asked for HE-AAC: those of a Mac's encoders are not there (AudioConverterNew says 'fmt?')";
export const CASES = [
  { name: 'formats', args: ['formats'], codec: 'mp4a.40.2', encode: true },
  { name: 'formats-more', args: ['formats-more'], codec: 'mp4a.40.2', encode: true, differs: LC_ONLY },
  { name: 'encode-aac', args: ['encode', 'aac', '44100', '2'], codec: 'mp4a.40.2', encode: true },
  { name: 'encode-aac-mono', args: ['encode', 'aac', '48000', '1', '30000'], codec: 'mp4a.40.2', encode: true },
  { name: 'encode-aac-96k', args: ['encode', 'aac', '48000', '2', '48000', '0', '96000'], codec: 'mp4a.40.2', encode: true },
  { name: 'encode-he-aac', args: ['encode', 'aach', '44100', '2'], codec: 'mp4a.40.5', encode: true, differs: LC_ONLY },
  { name: 'encode-opus', args: ['encode', 'opus', '48000', '2'], codec: 'opus', encode: true },
  { name: 'encode-opus-2.5ms', args: ['encode', 'opus', '48000', '2', '48000', '120'], codec: 'opus', encode: true },
  { name: 'encode-opus-mono-60ms', args: ['encode', 'opus', '48000', '1', '20000', '2880', '64000'], codec: 'opus', encode: true },
  { name: 'encode-opus-24k', args: ['encode', 'opus', '24000', '2'], codec: 'opus', encode: true },
  { name: 'decode-he-aac', make: ['encode', 'aach', '44100', '2'], packets: 'he.atpk', codec: 'mp4a.40.5' },
  { name: 'decode-he-aac-v2', make: ['encode', 'aacp', '44100', '2'], packets: 'he2.atpk', codec: 'mp4a.40.29' },
  { name: 'decode-aac-22k', make: ['encode', 'aac', '22050', '1'], packets: 'lc22.atpk', codec: 'mp4a.40.2' },
  { name: 'decode-opus-mac', make: ['encode', 'opus', '48000', '2', '48000', '480'], packets: 'opus.atpk', codec: 'opus' },
  { name: 'decode-mp3', args: ['decode', 'sweep.mp3'], file: true, codec: 'mp3' },
  { name: 'decode-mp3-22k', args: ['decode', 'sweep-22k.mp3'], file: true, codec: 'mp3' },
  // ExtAudioFile over the same codecs: MP3 files read, CAF files of AAC
  // and Opus written (`out`, beside the packets) and read back.
  { name: 'file-mp3', args: ['read', 'sweep.mp3'], file: true, codec: 'mp3' },
  { name: 'file-mp3-22k', args: ['read', 'sweep-22k.mp3'], file: true, codec: 'mp3' },
  { name: 'file-aac-caf', args: ['write', 'aac', '44100', '2'], out: 'w-aac.caf', codec: 'mp4a.40.2', encode: true },
  { name: 'file-opus-caf', args: ['write', 'opus', '48000', '2'], out: 'w-opus.caf', codec: 'opus', encode: true },
];
export const SOUNDS = ['sweep.mp3', 'sweep-22k.mp3'];
/** A case's arguments, its sound found in `dir` (and its packets in `packets`). */
export const argsOf = (c, dir, packets = dir) =>
  c.packets ? ['decode-packets', `${packets}/${c.packets}`] : c.out ? [...c.args, `${packets}/${c.out}`] : c.file ? [c.args[0], `${dir}/${c.args[1]}`, ...c.args.slice(2)] : c.args;
