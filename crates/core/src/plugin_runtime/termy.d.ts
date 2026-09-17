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

type TermyPluginSelectOption = {
  value: string;
  label: string;
  keywords?: string[];
  status?: string;
};

type TermyPluginPickInput<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> = {
  id: string;
  type: "pick";
  label: string;
  placeholder?: string;
  defaultValue?: string;
  required?: boolean;
  loadOptions(request: {
    query: string;
    context: TermyPluginContext<T>;
  }): TermyPluginSelectOption[] | Promise<TermyPluginSelectOption[]>;
};

type TermyPluginConfirmInput = {
  id: string;
  type: "confirm";
  label: string;
  defaultValue?: boolean;
};

type TermyPluginInput<
  T extends Record<string, TermyPluginSetting> = Record<string, TermyPluginSetting>,
> =
  | TermyPluginTextInput
  | TermyPluginSelectInput
  | TermyPluginPickInput<T>
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
  readonly origin: {
    readonly windowId: string;
    readonly tabId?: string;
    readonly paneId?: string;
  };
  readonly workingDirectory?: string;
  readonly activeCommand?: string;
  readonly selectedText?: string;
  readonly selectedTextTruncated: boolean;
  readonly shell: string;
  readonly runtime: "native" | "tmux";
  readonly activeTab?: {
    readonly id: string;
    readonly index: number;
    readonly title: string;
    readonly paneCount: number;
  };
  readonly activePane?: {
    readonly id: string;
    readonly index: number;
    readonly kind: "terminal";
  };
  readonly platform: "macos" | "linux" | "windows";
  readonly appVersion: string;
  readonly settings: TermyPluginSettings<T>;
  readonly toasts: TermyPluginToasts;
  readonly signal: AbortSignal;
  readonly progress: {
    report(update: { message?: string; percentage?: number }): void;
  };
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
  readonly TextArea: TermyUiLeafComponent<{
    id: string;
    label?: string;
    placeholder?: string;
    value?: string;
    maxLength?: number;
    rows?: number;
    submit?: string;
    disabled?: boolean;
  }>;
  readonly Select: TermyUiLeafComponent<{
    id: string;
    label?: string;
    placeholder?: string;
    value?: string;
    options: TermyPluginSelectOption[];
    action?: string;
    disabled?: boolean;
  }>;
  readonly List: TermyUiContainerComponent<{
    id: string;
    action?: string;
    selectedId?: string;
    searchPlaceholder?: string;
    filtering?: boolean;
    isLoading?: boolean;
  }>;
  readonly ListItem: TermyUiLeafComponent<{
    id: string;
    title: string;
    subtitle?: string;
    keywords?: string[];
    status?: string;
    payload?: string;
    action?: string;
    disabled?: boolean;
  }>;
  readonly EmptyState: TermyUiLeafComponent<{
    title: string;
    description?: string;
  }>;
  readonly Progress: TermyUiLeafComponent<{
    label?: string;
    value?: number;
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
    params: Readonly<Record<string, TermyPluginJsonValue>>;
    context: TermyPluginContext<T>;
  }): TermyUiChild | Promise<TermyUiChild>;
  onAction?(request: {
    action: TermyPluginViewAction;
    values: Readonly<Record<string, TermyPluginViewValue>>;
    params: Readonly<Record<string, TermyPluginJsonValue>>;
    context: TermyPluginContext<T>;
  }): TermyPluginResult | Promise<TermyPluginResult>;
};

type TermyPluginTerminalTarget =
  | "origin"
  | "active"
  /** Exact IDs are scoped to the Termy window that delivered the invocation. */
  | { windowId: string; tabId?: string; paneId?: string };

type TermyPluginTerminalLaunch =
  | { type: "shell"; command: string }
  | { type: "program"; program: string; args?: string[] };

type TermyPluginAction =
  /** @deprecated Use terminal.sendText or terminal.open for explicit behavior. */
  | { type: "terminal.run"; command: string; workingDirectory?: string }
  | {
      type: "terminal.sendText";
      text: string;
      submit?: boolean;
      target?: TermyPluginTerminalTarget;
    }
  | {
      type: "terminal.open";
      location?: "tab" | "splitRight" | "splitDown" | "window";
      workingDirectory?: string;
      launch?: TermyPluginTerminalLaunch;
      target?: TermyPluginTerminalTarget;
      /** Restores prior focus for tabs/splits. New OS windows receive focus. */
      focus?: boolean;
    }
  | { type: "termy.command"; command: string }
  | { type: "clipboard.write"; text: string }
  | { type: "url.open"; url: string }
  /** Requires `"native-ui"` in plugin.json capabilities. */
  | {
      type: "view.open";
      view: string;
      target?: "modal" | "commandPalette";
      params?: Record<string, TermyPluginJsonValue>;
    }
  | {
      type: "view.replace";
      view: string;
      params?: Record<string, TermyPluginJsonValue>;
    }
  | { type: "view.close" }
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
  | {
      readonly type: "tab.activated";
      readonly previousTabIndex?: number;
      readonly previousTabId?: string;
    }
  | { readonly type: "pane.activated"; readonly previousPaneId?: string }
  | { readonly type: "tab.created"; readonly tabId: string }
  | { readonly type: "tab.closed"; readonly tabId: string }
  | { readonly type: "terminal.bell" }
  | {
      readonly type: "workingDirectory.changed";
      readonly previousWorkingDirectory?: string;
      readonly workingDirectory?: string;
    }
  | {
      readonly type: "command.started";
      readonly command?: string;
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
  inputs?: TermyPluginInput<T>[];
  when?: {
    hasSelection?: boolean;
    hasWorkingDirectory?: boolean;
    runtimes?: ("native" | "tmux")[];
    platforms?: ("macos" | "linux" | "windows")[];
  };
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
