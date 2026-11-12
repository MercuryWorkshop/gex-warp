import type { System } from './index';

/** The application a program says it is. */
export interface NotificationApp {
  /** Its identifier (a Mac bundle identifier), up to 256 bytes; '' when it gave none. */
  readonly id: string;
  readonly name: string;
  /** Its icon (PNG, JPEG, GIF or WebP, at most 256 KiB), or null. */
  readonly icon: Blob | null;
}

export interface NotificationAction {
  readonly id: string;
  readonly title: string;
  readonly destructive: boolean;
  /** Choosing it brings the application forward. */
  readonly foreground: boolean;
  /** It wants the user's authentication first (the shell's to ask, or ignore). */
  readonly authenticate: boolean;
  /** It takes typed text: the button's title and the field's placeholder. */
  readonly text: { readonly button: string; readonly placeholder: string } | null;
}

/** A notification a program posted: frozen data, every string the program's (bounded, never markup), and how to answer it. */
export declare class GuestNotification {
  /** Unique on the page. */
  readonly key: string;
  /** The program's identifier for it (a later post of the same one replaces it). */
  readonly id: string;
  readonly pid: number;
  readonly app: NotificationApp;
  readonly title: string;
  readonly subtitle: string;
  readonly body: string;
  /** A sound to play (none is played by the runtime): the system's default, one by name, or none. */
  readonly sound: 'default' | { readonly name: string } | null;
  /** The application's badge this notification sets, or null for no change. */
  readonly badge: number | null;
  /** The group it belongs with ('' none), and its category. */
  readonly thread: string;
  readonly category: string;
  readonly actions: readonly NotificationAction[];
  /** Its image attachment (at most 2 MiB), or null. */
  readonly image: Blob | null;
  /** Show it as a banner now; keep it in a list. */
  readonly banner: boolean;
  readonly list: boolean;
  readonly interruption: 'passive' | 'active' | 'time-sensitive' | 'critical';
  /** When the page was given it. */
  readonly date: Date;
  /** It replaces an outstanding one of the same `id` (a `withdrawn` with reason `replaced` came first). */
  readonly replaces: boolean;
  readonly outstanding: boolean;
  /** The user chose it (no `actionId`: the notification itself), with typed `text` for an action that takes text. False if it is no longer outstanding. */
  activate(actionId?: string | null, options?: { text?: string }): boolean;
  /** The user closed it. */
  dismiss(): boolean;
  /** The shell took it away without an answer. */
  clear(): boolean;
  /** The user asked for the application's notification settings from it. */
  openSettings(): boolean;
}

export type NotificationDecision = boolean | 'provisional' | { alert?: boolean; sound?: boolean; badge?: boolean; provisional?: boolean };

export interface AuthorizationRequest {
  readonly pid: number;
  readonly app: NotificationApp;
  readonly options: readonly ('alert' | 'sound' | 'badge' | 'provisional' | 'critical' | 'settings')[];
}

export interface DecisionState {
  readonly status: 'authorized' | 'denied' | 'provisional';
  readonly alert: boolean;
  readonly sound: boolean;
  readonly badge: boolean;
}

/**
 * The page's shell presents programs' notifications (the `notifications` service of /dev/gex-services). One per system, made any time.
 * Events: `notification` (`notification`), `withdrawn` (`notification`, `reason`: 'program' | 'replaced' | 'answered' | 'cleared' | 'limit' | 'gone'), `badge` (`app`, `pid`, `count`, `label`), `connect` and `disconnect` (`app`, `pid`).
 */
export declare class Notifications extends EventTarget {
  /** `authorize` decides when an application first asks (kept per application for the system's life); without it, every one is allowed. */
  constructor(sys: System, options?: { authorize?: (request: AuthorizationRequest) => NotificationDecision | Promise<NotificationDecision> });
  /** What is outstanding, oldest first: every program's, or one process's. */
  list(pid?: number): GuestNotification[];
  apps(): { pid: number; app: NotificationApp }[];
  decision(appId: string): DecisionState | null;
  /** Change (or with null forget) an application's decision; its connected programs are told. */
  setDecision(appId: string, decision: NotificationDecision | null): void;
  /** The user asked for the application's notification settings. */
  openSettings(appId: string): void;
}

export declare const LIMITS: {
  readonly icon: number; readonly image: number; readonly id: number; readonly title: number; readonly body: number;
  readonly actions: number; readonly outstanding: number; readonly badgeLabel: number;
};
