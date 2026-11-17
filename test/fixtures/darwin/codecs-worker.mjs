// darwin/codecs-page's Worker: the `codecs` service served off the page's
// thread (page/runtime codecs-worker.js).
import { serveCodecs } from '../../../page/runtime/src/codecs-worker.js';

serveCodecs();
