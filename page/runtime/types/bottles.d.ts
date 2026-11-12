import type { System, Process, SpawnOptions, RunOptions, RunResult, Env } from './index';

/** A bottle's own settings; each is there only when set. */
export interface BottleSettings {
  name?: string;
  /** The system layer: a Mac root's directory in the system's tree. */
  system?: string;
  /** Made with the bottle: `gethostuuid`, and the serial number. */
  uuid?: string;
  serial?: string;
  /** A release of the layer's table: '14', '14.8.9', 'sonoma'. */
  macos?: string;
  /** A model of the layer's table: 'Mac14,3'. */
  machine?: string;
  /** 'honest', 'machine', or a GPU family ('mac2', 'apple7', 'apple8'). Stored; nothing acts on it yet. */
  graphics?: string;
  /** 'no': the system outside is not the volume /Volumes/Linux. */
  'share.system'?: string;
  /** A directory outside that is the user's home. */
  'share.home'?: string;
}

/** What `set` takes: a null or empty value forgets the key; `share.system` may be a boolean. */
export type BottleChanges = { [K in keyof BottleSettings]?: string | boolean | null };
export type AppSettings = Pick<BottleSettings, 'macos' | 'machine' | 'graphics'>;

export interface Bottle {
  name: string;
  /** Its directory in the system's tree. */
  dir: string;
  settings: BottleSettings;
  apps: Record<string, AppSettings>;
}

/** A row of the system layer's table of Macs; null what is not known. */
export interface MacModel {
  model: string; arch: 'arm64' | 'x86_64';
  target: string | null; targettype: string | null; soc: string | null;
  cpufamily: string | null; cpusubfamily: string | null;
  pcores: string | null; ecores: string | null; threads: string | null;
  brand: string | null; first: string | null; last: string | null;
  gpufamily: string | null; name: string | null;
}

export interface MacRelease {
  product: string; build: string;
  darwin: string | null; xnu: string | null; kdate: string | null; compat: string | null; ios: string | null; year: string | null;
}

export declare const CONF: string;
export declare const PARTS: string[];
export declare const KEYS: string[];
export declare const APP_KEYS: string[];
export declare function parseConf(text: string): { settings: Record<string, string>; apps: Record<string, Record<string, string>> };
export declare function formatConf(conf: { settings: Record<string, string>; apps?: Record<string, Record<string, string>> }): string;

export declare class Bottles {
  constructor(sys: System, options?: { dir?: string; system?: string });
  readonly dir: string;
  readonly system: string;
  path(name: string): string;
  create(name: string, settings?: BottleChanges, options?: { system?: string }): Promise<Bottle>;
  list(): Promise<Bottle[]>;
  /** Rejects with `code` 'ENOENT' when there is none. */
  get(name: string): Promise<Bottle>;
  set(name: string, settings: BottleChanges, options?: { app?: string }): Promise<Bottle>;
  removeApp(name: string, app: string): Promise<Bottle>;
  delete(name: string): Promise<void>;
  machines(name?: string): Promise<MacModel[]>;
  releases(name?: string): Promise<MacRelease[]>;
  outer(name: string, path: string): Promise<string>;
  env(name: string, env?: Env): Promise<Record<string, string>>;
  spawn(name: string, argv: string[], options?: SpawnOptions): Promise<Process>;
  run(name: string, argv: string[], options?: RunOptions & { encoding?: string }): Promise<RunResult<string>>;
  run(name: string, argv: string[], options: RunOptions & { encoding: null }): Promise<RunResult<Uint8Array>>;
}
