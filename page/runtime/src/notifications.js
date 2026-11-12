// `Notifications`: programs' user notifications, presented by the page's
// shell. The first service of /dev/gex-services (services.js): a program's
// framework (pyrus's UserNotifications and Foundation's
// NSUserNotificationCenter; a Linux or Windows bridge later) connects to
// the service `notifications`, says which application it is, asks to be
// allowed, posts, withdraws and sets its badge; this checks every message
// (all of it is the program's word: text is bounded and never interpreted,
// images are size-checked and sniffed), hands the shell plain frozen data,
// and tells the program what the shell decided and what the user did. The
// README's Notifications section is the reference for the messages.

import { internals } from './internal.js';

/** What a shell takes of what a program sends (hosts/native notify.rs has
 *  the same). */
export const LIMITS = Object.freeze({
  icon: 256 << 10, image: 2 << 20, id: 256, title: 1024, body: 8192, actions: 10, outstanding: 100, badgeLabel: 16,
});

const OPTIONS = ['alert', 'sound', 'badge', 'provisional', 'critical', 'settings'];

/** `s` with control characters other than a line break and a tab dropped,
 *  cut to `max` bytes of UTF-8 at a character's end. Not a string: ''. */
export function cleanText(s, max) {
  if (typeof s !== 'string') return '';
  let out = '', bytes = 0;
  for (const ch of s) {
    const c = ch.codePointAt(0);
    if ((c < 0x20 && c !== 10 && c !== 9) || (c >= 0x7f && c < 0xa0)) continue;
    const n = c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
    if (bytes + n > max) break;
    bytes += n;
    out += ch;
  }
  return out;
}

/** The type an image's bytes say they are, or null. */
export function sniffImage(b) {
  if (b.length >= 8 && [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every((v, i) => b[i] === v)) return 'image/png';
  if (b.length >= 3 && b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return 'image/jpeg';
  const a = (i, n) => String.fromCharCode(...b.subarray(i, i + n));
  if (b.length >= 6 && (a(0, 6) === 'GIF87a' || a(0, 6) === 'GIF89a')) return 'image/gif';
  if (b.length >= 12 && a(0, 4) === 'RIFF' && a(8, 4) === 'WEBP') return 'image/webp';
  return null;
}

/** The image `ref` (`{at, size}`) names in a frame's `data`, as a Blob, if
 *  it is one a shell takes; else null. */
function imageOf(ref, data, max) {
  if (!ref || typeof ref !== 'object') return null;
  const { at, size } = ref;
  if (!Number.isInteger(at) || !Number.isInteger(size) || at < 0 || size <= 0 || size > max || at + size > data.length) return null;
  const bytes = data.slice(at, at + size);
  const type = sniffImage(bytes);
  return type ? new Blob([bytes], { type }) : null;
}

/** A decision as the shell may give it, as the program is told it. */
function decided(d) {
  if (d && typeof d === 'object') {
    const on = { alert: !!d.alert, sound: !!d.sound, badge: !!d.badge };
    return { status: d.provisional ? 'provisional' : on.alert || on.sound || on.badge ? 'authorized' : 'denied', ...on };
  }
  if (d === 'provisional') return { status: 'provisional', alert: true, sound: false, badge: true };
  if (d === true || d === 'granted' || d === 'authorized') return { status: 'authorized', alert: true, sound: true, badge: true };
  return { status: 'denied', alert: false, sound: false, badge: false };
}

/** One notification a program posted, as the shell is given it. */
export class GuestNotification {
  #answer;
  #outstanding = true;

  constructor(fields, answer) {
    Object.assign(this, fields);
    this.#answer = answer;
    Object.freeze(this);
  }

  /** Whether it is still the shell's to show (not answered, withdrawn,
   *  replaced or cleared). */
  get outstanding() {
    return this.#outstanding;
  }

  /** The user chose it: its default action (a click on it), or the action
   *  `actionId` of its `actions`, with the `text` typed for one that takes
   *  text. The program is told; it is no longer outstanding. An unknown
   *  action is the default one. */
  activate(actionId = null, { text } = {}) {
    const action = actionId != null && this.actions.some((a) => a.id === actionId) ? actionId : 'default';
    const m = { op: 'response', id: this.id, action };
    if (text != null && this.actions.find((a) => a.id === action)?.text) m.text = cleanText(String(text), LIMITS.body);
    return this.#answer(m, true);
  }

  /** The user closed it. */
  dismiss() {
    return this.#answer({ op: 'response', id: this.id, action: 'dismiss' }, true);
  }

  /** The shell took it away without the user answering (expired, cleared
   *  with others). */
  clear() {
    return this.#answer({ op: 'cleared', id: this.id }, true);
  }

  /** The user asked for the application's notification settings from it.
   *  It stays outstanding. */
  openSettings() {
    return this.#answer({ op: 'settings', id: this.id }, false);
  }

  /** (The service's.) */
  _gone() {
    this.#outstanding = false;
  }
}

/**
 * The page's shell presents programs' notifications. One per system; made
 * any time.
 *
 * `authorize(request)` decides whether an application may notify, when one
 * first asks (the decision is kept for the system's life, per application,
 * as a Mac keeps it): `request` is `{ pid, app, options }` (frozen; `app`
 * as a notification's, `options` what it asks for: `'alert'`, `'sound'`,
 * `'badge'`, `'provisional'`, `'critical'`). It returns or resolves `true`,
 * `false`, `'provisional'` or `{ alert, sound, badge }`; a throw is a
 * denial. Without it every application is allowed alerts, sounds and
 * badges.
 *
 * Events: `notification` (`notification`, a GuestNotification), `withdrawn`
 * (`notification`, `reason`: `'program'`, `'replaced'`, `'answered'`,
 * `'cleared'`, `'limit'` or `'gone'`), `badge` (`app`, `pid`, `count`,
 * `label`), `connect` and `disconnect` (`app`, `pid`).
 */
export class Notifications extends EventTarget {
  #authorize;
  #decisions = new Map();   // app id -> the program's form of the decision
  #conns = new Set();       // per connection: { conn, app, pid, out: Map of its id -> GuestNotification }
  #seq = 0;

  constructor(sys, { authorize = null } = {}) {
    super();
    if (authorize != null && typeof authorize !== 'function') throw new TypeError('Notifications: `authorize` is a function');
    const i = internals(sys, 'Notifications');
    i.attach('notifications', this);
    this.#authorize = authorize;
    sys.services.provide('notifications', { version: 1, open: (conn) => this.#open(conn) });
  }

  /** The notifications outstanding, oldest first: every program's, or
   *  process `pid`'s. */
  list(pid) {
    const out = [];
    for (const c of this.#conns) if (pid == null || c.pid === pid) out.push(...c.out.values());
    return out;
  }

  /** The applications connected: `[{ pid, app }]`. */
  apps() {
    return [...this.#conns].filter((c) => c.app).map((c) => ({ pid: c.pid, app: c.app }));
  }

  /** The decision kept for application `appId` (`{ status, alert, sound,
   *  badge }`), or null when none was made. */
  decision(appId) {
    return this.#decisions.get(appId) ?? null;
  }

  /** Change the decision for `appId` (as `authorize` answers; null forgets
   *  it): its programs that are connected are told. */
  setDecision(appId, d) {
    if (d == null) this.#decisions.delete(appId);
    else this.#decisions.set(appId, decided(d));
    const now = this.#decisions.get(appId) ?? { status: 'not-determined', alert: false, sound: false, badge: false };
    for (const c of this.#conns) if (c.app?.id === appId) c.conn.send({ op: 'authorization', ...now });
  }

  /** The user asked for `appId`'s notification settings, from no
   *  notification in particular. */
  openSettings(appId) {
    for (const c of this.#conns) if (c.app?.id === appId) c.conn.send({ op: 'settings', id: null });
  }

  #emit(type, props) {
    this.dispatchEvent(Object.assign(new Event(type), props));
  }

  #open(conn) {
    const c = { conn, app: null, pid: conn.pid, out: new Map(), badge: false };
    this.#conns.add(c);
    conn.addEventListener('message', ({ message, data }) => {
      try {
        this.#message(c, message, data);
      } catch (e) {
        console.error('[gex] notifications: a message the service could not take:', e);
      }
    });
    conn.addEventListener('close', () => {
      this.#conns.delete(c);
      for (const n of c.out.values()) this.#withdraw(c, n, 'gone');
      if (c.badge && c.app) this.#emit('badge', { app: c.app, pid: c.pid, count: 0, label: null });
      if (c.app) this.#emit('disconnect', { app: c.app, pid: c.pid });
    });
  }

  #withdraw(c, n, reason) {
    if (c.out.get(n.id) === n) c.out.delete(n.id);
    n._gone();
    this.#emit('withdrawn', { notification: n, reason });
  }

  async #message(c, m, data) {
    const op = m.op;
    if (!c.app && op !== 'hello') return;
    if (op === 'hello') {
      if (c.app) return;
      const a = m.app && typeof m.app === 'object' ? m.app : {};
      c.app = Object.freeze({ id: cleanText(a.id, LIMITS.id), name: cleanText(a.name, LIMITS.title), icon: imageOf(a.icon, data, LIMITS.icon) });
      c.conn.send({ op: 'welcome', limits: { icon: LIMITS.icon, image: LIMITS.image, title: LIMITS.title, body: LIMITS.body, actions: LIMITS.actions, outstanding: LIMITS.outstanding } });
      this.#emit('connect', { app: c.app, pid: c.pid });
    } else if (op === 'authorize') {
      const seq = Number.isInteger(m.seq) ? m.seq : 0;
      let d = this.#decisions.get(c.app.id);
      if (!d && m.ask === true) {
        const options = Object.freeze((Array.isArray(m.options) ? m.options : []).filter((o) => OPTIONS.includes(o)));
        let answer = true;
        if (this.#authorize) {
          try {
            answer = await this.#authorize(Object.freeze({ pid: c.pid, app: c.app, options }));
          } catch {
            answer = false;
          }
        }
        // (Another of its programs may have been answered meanwhile.)
        d = this.#decisions.get(c.app.id) ?? decided(answer);
        this.#decisions.set(c.app.id, d);
      }
      c.conn.send({ op: 'authorization', seq, ...(d ?? { status: 'not-determined', alert: false, sound: false, badge: false }) });
    } else if (op === 'post') {
      this.#post(c, m, data);
    } else if (op === 'withdraw') {
      const all = m.all === true;
      const ids = new Set(Array.isArray(m.ids) ? m.ids.filter((x) => typeof x === 'string') : []);
      for (const n of [...c.out.values()]) if (all || ids.has(n.id)) this.#withdraw(c, n, 'program');
    } else if (op === 'badge') {
      const label = typeof m.label === 'string' ? cleanText(m.label, LIMITS.badgeLabel) : null;
      const count = label == null && Number.isFinite(m.count) ? Math.max(0, Math.min(1e9, Math.floor(m.count))) : null;
      c.badge = !!(label || count);
      this.#emit('badge', { app: c.app, pid: c.pid, count: label == null ? count ?? 0 : null, label });
    }
  }

  #post(c, m, data) {
    const id = cleanText(m.id, LIMITS.id);
    const d = this.#decisions.get(c.app.id);
    if (!d || d.status === 'denied') {
      c.conn.send({ op: 'refused', id, reason: 'not authorized' });
      return;
    }
    const earlier = c.out.get(id);
    const replaces = !!earlier && m.replace !== false;
    if (earlier) this.#withdraw(c, earlier, 'replaced');
    if (c.out.size >= LIMITS.outstanding) {
      const oldest = c.out.values().next().value;
      this.#withdraw(c, oldest, 'limit');
      c.conn.send({ op: 'cleared', id: oldest.id });
    }
    const t = (k, max) => cleanText(m[k], max);
    const sound = m.sound === 'default' ? 'default' : typeof m.sound === 'string' && m.sound ? Object.freeze({ name: cleanText(m.sound, LIMITS.id) }) : null;
    const actions = (Array.isArray(m.actions) ? m.actions : []).filter((a) => a && typeof a === 'object').slice(0, LIMITS.actions).map((a) => Object.freeze({
      id: cleanText(a.id, LIMITS.id),
      title: cleanText(a.title, LIMITS.id),
      destructive: a.destructive === true,
      foreground: a.foreground === true,
      authenticate: a.authenticate === true,
      text: a.text && typeof a.text === 'object' ? Object.freeze({ button: cleanText(a.text.button, LIMITS.id), placeholder: cleanText(a.text.placeholder, LIMITS.title) }) : null,
    })).filter((a) => a.id && a.id !== 'default' && a.id !== 'dismiss');
    const key = `${c.pid}:${++this.#seq}`;
    const n = new GuestNotification({
      key, id, pid: c.pid, app: c.app,
      title: t('title', LIMITS.title), subtitle: t('subtitle', LIMITS.title), body: t('body', LIMITS.body),
      sound, badge: Number.isFinite(m.badge) ? Math.max(0, Math.min(1e9, Math.floor(m.badge))) : null,
      thread: t('thread', LIMITS.id), category: t('category', LIMITS.id), actions: Object.freeze(actions),
      image: imageOf(m.image, data, LIMITS.image),
      banner: m.banner === true, list: m.list !== false,
      interruption: ['passive', 'active', 'time-sensitive', 'critical'].includes(m.interruption) ? m.interruption : 'active',
      date: new Date(), replaces,
    }, (message, ends) => {
      if (c.out.get(id) !== n) return false;
      if (ends) this.#withdraw(c, n, message.op === 'cleared' ? 'cleared' : 'answered');
      return c.conn.send(message);
    });
    c.out.set(id, n);
    c.conn.send({ op: 'shown', id, presented: true });
    this.#emit('notification', { notification: n });
  }
}
