// Managed ambient declarations for plain TypeScript plugins.
type TermyPluginIcon =
  | "command"
  | "play"
  | "terminal"
  | "folder"
  | "link"
  | "clipboard"
  | "settings"
  | "info";

type TermyPluginInputValue = string | boolean;

type TermyPluginTextInput = {
  id: string;
  type: "text";
  label: string;
  placeholder?: string;
  defaultValue?: string;
  required?: boolean;
  maxLength?: number;
};

type TermyPluginSelectInput = {
  id: string;
  type: "select";
  label: string;
  placeholder?: string;
  defaultValue?: string;
  required?: boolean;
  options: Array<{
    value: string;
    label: string;
    keywords?: string[];
    status?: string;
  }>;
};

type TermyPluginConfirmInput = {
  id: string;
  type: "confirm";
  label: string;
  defaultValue?: boolean;
};

type TermyPluginInput =
  | TermyPluginTextInput
  | TermyPluginSelectInput
  | TermyPluginConfirmInput;

type TermyPluginToasts = {
  info(message: string): void;
  success(message: string): void;
  warning(message: string): void;
  error(message: string): void;
};

type TermyPluginJsonValue =
  | null
  | boolean
  | number
  | string
  | TermyPluginJsonValue[]
  | { [key: string]: TermyPluginJsonValue };

type TermyPluginStorage = {
  get<T = TermyPluginJsonValue>(key: string): Promise<T | undefined>;
  set(key: string, value: TermyPluginJsonValue): Promise<void>;
  delete(key: string): Promise<boolean>;
  clear(): Promise<void>;
};

type TermyPluginToggleSetting = {
  type: "toggle";
  title: string;
  description?: string;
  defaultValue?: boolean;
};

type TermyPluginTextSetting = {
  type: "text";
  title: string;
  description?: string;
  placeholder?: string;
  defaultValue?: string;
  maxLength?: number;
};

type TermyPluginSelectSetting = {
  type: "select";
  title: string;
  description?: string;
  defaultValue?: string;
  options: Array<{ value: string; label: string }>;
};

type TermyPluginSecretSetting = {
  type: "secret";
  title: string;
  description?: string;
  placeholder?: string;
  maxLength?: number;
};

type TermyPluginSetting =
  | TermyPluginToggleSetting
  | TermyPluginTextSetting
  | TermyPluginSelectSetting
  | TermyPluginSecretSetting;

type TermyPluginSettingValue<T extends TermyPluginSetting> =
  T extends TermyPluginToggleSetting ? boolean : string;

type TermyPluginSettings<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  get<K extends keyof T & string>(key: K): TermyPluginSettingValue<T[K]> | undefined;
};

type TermyPluginContext<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  readonly workingDirectory?: string;
  readonly activeCommand?: string;
  readonly selectedText?: string;
  readonly selectedTextTruncated: boolean;
  readonly shell: string;
  readonly runtime: "native" | "tmux";
  readonly activeTab?: {
    readonly index: number;
    readonly title: string;
    readonly paneCount: number;
  };
  readonly activePane?: {
    readonly index: number;
    readonly kind: "terminal" | "browser";
  };
  readonly platform: "macos" | "linux" | "windows";
  readonly appVersion: string;
  readonly settings: TermyPluginSettings<T>;
  readonly toasts: TermyPluginToasts;
  /** Requires `"storage"` in plugin.json capabilities. */
  readonly storage: TermyPluginStorage;
  /** Requires `"storage"` in plugin.json capabilities. */
  readonly paths: {
    readonly dataDirectory: string;
    readonly cacheDirectory: string;
  };
};

type TermyUiGap = "none" | "small" | "medium" | "large";
type TermyUiAlignment = "start" | "center" | "end" | "stretch";
type TermyUiTone = "default" | "muted" | "success" | "danger";
type TermyUiTextVariant = "heading" | "body" | "caption" | "code";
type TermyUiButtonVariant = "secondary" | "primary" | "danger";

type TermyUiElement = { readonly __termyUiElement?: never };
type TermyUiChild =
  | TermyUiElement
  | string
  | number
  | boolean
  | null
  | undefined
  | TermyUiChild[];
type TermyUiTextChild = string | number | TermyUiTextChild[];
type TermyUiKey = { key?: string | number };
type TermyUiContainerComponent<P> = (
  props: P & TermyUiKey & { children?: TermyUiChild },
) => TermyUiElement;
type TermyUiTextComponent<P> = (
  props: P & TermyUiKey & { children: TermyUiTextChild },
) => TermyUiElement;
type TermyUiLeafComponent<P> = (
  props: P & TermyUiKey & { children?: never },
) => TermyUiElement;

declare const TermyUI: {
  createElement(
    component: ((props: any) => TermyUiElement) | symbol,
    props: Record<string, unknown> | null,
    ...children: TermyUiChild[]
  ): TermyUiElement;
  readonly Fragment: symbol;
  readonly Column: TermyUiContainerComponent<{
    gap?: TermyUiGap;
    align?: TermyUiAlignment;
  }>;
  readonly Row: TermyUiContainerComponent<{
    gap?: TermyUiGap;
    align?: TermyUiAlignment;
  }>;
  readonly Text: TermyUiTextComponent<{
    variant?: TermyUiTextVariant;
    tone?: TermyUiTone;
  }>;
  readonly TextInput: TermyUiLeafComponent<{
    id: string;
    label?: string;
    placeholder?: string;
    value?: string;
    maxLength?: number;
    submit?: string;
    disabled?: boolean;
  }>;
  readonly Button: TermyUiTextComponent<{
    id: string;
    action: string;
    payload?: string;
    variant?: TermyUiButtonVariant;
    disabled?: boolean;
  }>;
  readonly Checkbox: TermyUiTextComponent<{
    id: string;
    action: string;
    payload?: string;
    checked?: boolean;
    disabled?: boolean;
  }>;
  readonly Divider: TermyUiLeafComponent<Record<never, never>>;
  readonly Spacer: TermyUiLeafComponent<{ size?: TermyUiGap }>;
};

declare namespace JSX {
  type Element = TermyUiElement;
  interface ElementChildrenAttribute {
    children: {};
  }
}

type TermyPluginViewValue = string | boolean;
type TermyPluginViewAction = {
  readonly id: string;
  readonly controlId: string;
  readonly payload?: string;
  readonly value?: TermyPluginViewValue;
};

type TermyPluginView<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  title: string;
  timeoutMs?: number;
  render(request: {
    context: TermyPluginContext<T>;
  }): TermyUiChild | Promise<TermyUiChild>;
  onAction?(request: {
    action: TermyPluginViewAction;
    values: Readonly<Record<string, TermyPluginViewValue>>;
    context: TermyPluginContext<T>;
  }): TermyPluginResult | Promise<TermyPluginResult>;
};

type TermyPluginAction =
  | { type: "terminal.run"; command: string; workingDirectory?: string }
  | { type: "termy.command"; command: string }
  | { type: "clipboard.write"; text: string }
  | { type: "url.open"; url: string }
  /** Requires `"native-ui"` in plugin.json capabilities. */
  | {
      type: "view.open";
      view: string;
      target?: "modal" | "commandPalette";
    }
  | {
      type: "toast";
      level: "info" | "success" | "warning" | "error";
      message: string;
    };

type TermyPluginResult =
  | void
  | TermyPluginAction
  | TermyPluginAction[]
  | { actions: TermyPluginAction[] };

type TermyPluginEvent =
  | { readonly type: "terminal.ready" }
  | { readonly type: "tab.activated"; readonly previousTabIndex?: number }
  | {
      readonly type: "workingDirectory.changed";
      readonly previousWorkingDirectory?: string;
      readonly workingDirectory?: string;
    }
  | {
      readonly type: "command.finished";
      readonly command?: string;
      readonly exitCode?: number;
      readonly durationMs?: number;
    };

type TermyPluginEvents<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  readonly [K in TermyPluginEvent["type"]]?: (request: {
    readonly event: Extract<TermyPluginEvent, { type: K }>;
    readonly context: TermyPluginContext<T>;
  }) => TermyPluginResult | Promise<TermyPluginResult>;
};

type TermyPluginCommand<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  id: string;
  title: string;
  /**
   * Surfaces that list this command. Defaults to `["commandPalette"]`.
   * Context menus are currently available on Linux and Windows.
   */
  placements?: (
    | "commandPalette"
    | "terminalContextMenu"
    | "tabContextMenu"
  )[];
  keywords?: string[];
  status?: string;
  enabled?: boolean;
  disabledReason?: string;
  icon?: TermyPluginIcon;
  inputs?: TermyPluginInput[];
  timeoutMs?: number;
  run(request: {
    inputs: Record<string, TermyPluginInputValue>;
    context: TermyPluginContext<T>;
  }): TermyPluginResult | Promise<TermyPluginResult>;
};

type TermyPlugin<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  settings?: T;
  commands: TermyPluginCommand<T>[];
  events?: TermyPluginEvents<T>;
  /** Requires `"native-ui"` in plugin.json capabilities. */
  views?: Record<string, TermyPluginView<T>>;
};

declare function definePlugin<const T extends Record<string, TermyPluginSetting>>(
  plugin: TermyPlugin<T>,
): TermyPlugin<T>;
