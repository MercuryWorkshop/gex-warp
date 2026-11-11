import type { System } from './index';

export interface WindowEnv extends Record<string, string> {
  XDG_RUNTIME_DIR: string;
  WAYLAND_DISPLAY: string;
  DISPLAY: string;
}

/** Events: `title`, `resized`, `position`, `restack` (`below`), `request`
 *  (`kind`, `edges`), `closed`. */
export declare class GuestWindow extends EventTarget {
  /** Its pixels, CSS-sized to the window; focusable. */
  readonly canvas: HTMLCanvasElement;
  readonly kind: 'toplevel' | 'popup' | 'subsurface';
  get pid(): number;
  get parent(): GuestWindow | null;
  /** The menu a shell shows for this window: its own, else its
   *  application's menu bar; null when there is none. */
  get menu(): GuestMenu | null;
  get title(): string;
  get appId(): string | null;
  get decorations(): 'host' | 'client' | 'none';
  get placed(): boolean;
  get x(): number;
  get y(): number;
  get width(): number;
  get height(): number;
  get states(): { maximized: boolean; fullscreen: boolean; resizing: boolean };
  resize(width: number, height: number): void;
  setStates(states: { maximized?: boolean; fullscreen?: boolean; resizing?: boolean }, width?: number, height?: number): void;
  place(x: number, y: number): void;
  focus(): void;
  close(): void;
}

/** A key equivalent: the key as `KeyboardEvent.key` names the unshifted
 *  key ("s", ",", "Enter", "ArrowUp", "F5", " "), and every modifier held
 *  with it. `logo` is the Command, Windows or Super key. */
export interface MenuShortcut {
  readonly key: string;
  readonly shift: boolean;
  readonly control: boolean;
  readonly alt: boolean;
  readonly logo: boolean;
}

/** An item's picture: `width` x `height` pixels of RGBA (not
 *  premultiplied), shown at that size over `scale`. `template`: a shape,
 *  drawn from its alpha alone in the text's colour. */
export interface MenuImage {
  readonly width: number;
  readonly height: number;
  readonly scale: number;
  readonly template: boolean;
  readonly data: Uint8ClampedArray;
}

export type MenuRole =
  | 'application' | 'file' | 'edit' | 'view' | 'window' | 'help'
  | 'about' | 'preferences' | 'services' | 'hide' | 'hide-others' | 'show-all' | 'quit'
  | 'new' | 'open' | 'close' | 'save' | 'print'
  | 'undo' | 'redo' | 'cut' | 'copy' | 'paste' | 'delete' | 'select-all'
  | 'minimize' | 'zoom' | 'fullscreen' | 'bring-all-to-front'
  | (string & {});

/** One item of a menu: plain, frozen data. */
export interface MenuItem {
  /** The program's number for it: the same item across changes. */
  readonly id: number;
  readonly title: string;
  readonly enabled: boolean;
  readonly hidden: boolean;
  readonly separator: boolean;
  /** Stands in for the item before it while exactly its shortcut's
   *  modifiers are held; a shell without alternates hides it. */
  readonly alternate: boolean;
  /** A label for the items after it. */
  readonly header: boolean;
  readonly state: 'off' | 'on' | 'mixed';
  readonly indent: number;
  readonly role: MenuRole | null;
  readonly shortcut: MenuShortcut | null;
  readonly tooltip: string | null;
  readonly image: MenuImage | null;
  /** The menu it opens, or null. */
  readonly submenu: readonly MenuItem[] | null;
}

/** One tree of menus a program handed to the page. Events: `change`,
 *  `closed`. */
export declare class GuestMenu extends EventTarget {
  get scope(): 'menu-bar' | 'dock' | 'window';
  /** The application's name. */
  get title(): string;
  get pid(): number;
  get windows(): GuestWindow[];
  get items(): readonly MenuItem[];
  item(id: number): MenuItem | null;
  /** Before showing a menu (`id`, or 0: the bar): its items once the
   *  program has brought them up to date, or after `timeout` ms (300). */
  openMenu(id?: number, options?: { timeout?: number }): Promise<readonly MenuItem[]>;
  closeMenu(id?: number): void;
  activate(id: number, options?: {
    modifiers?: { shift?: boolean; control?: boolean; alt?: boolean; logo?: boolean } | { shiftKey: boolean; ctrlKey: boolean; altKey: boolean; metaKey: boolean };
  }): void;
}

/** A shortcut as a menu shows it: "⇧⌘S" (`mac`) or "Ctrl+Shift+S" (`plus`). */
export declare function shortcutLabel(shortcut: MenuShortcut | null, options?: { style?: 'mac' | 'plus'; logo?: string }): string;

/** Each window handed to the page to frame and place. Made before boot.
 *  Events: `window` (`window`), `cursor` (`css`), `menu` (`menu`). */
export declare class HostWindows extends EventTarget {
  constructor(sys: System, options?: { width?: number; height?: number; scale?: number; menus?: boolean });
  menus(): GuestMenu[];
  get env(): WindowEnv;
  setOutputSize(width: number, height: number, scale?: number): void;
  windows(): GuestWindow[];
  get focused(): GuestWindow | null;
  keyEvent(e: KeyboardEvent, pressed: boolean): void;
}
