# Plugins

Termy plugins add commands to the command palette with a small `plugin.json` manifest and a plain TypeScript entrypoint. A plugin does not need a package or handwritten build step: put both files in its plugin directory and export one `definePlugin(...)` value from the TypeScript source.

## Requirements

Plugins run in a persistent external [Bun](https://bun.sh/) host. Install Bun before opening the command palette with plugins present, or set `TERMY_BUN_PATH` to the absolute path of a Bun executable.

A command handler is normal Bun code, not a restricted expression language. It can use async functions, `fetch`, `Bun.*`, Node-compatible standard-library APIs, files, subprocesses, network requests, and local relative TypeScript imports. Returned actions are the typed bridge back into Termy's native command palette, terminal, clipboard, browser, and toast UI.

Termy resolves Bun in this order:

1. `TERMY_BUN_PATH`
2. `bun` or `bun.exe` beside the Termy executable
3. `bun` on `PATH`
4. `$HOME/.bun/bin/bun` on macOS and Linux, or `%USERPROFILE%\.bun\bin\bun.exe` on Windows
5. `/opt/homebrew/bin/bun`
6. `/usr/local/bin/bun`

Termy starts one long-lived Bun host and gives each plugin its own Worker. This keeps command invocation warm while allowing a timed-out or failed plugin Worker to be stopped without taking down the terminal UI.

## Plugin directory

The easiest install path is **Settings → Plugins → Install from folder**. Choose a plugin folder containing `plugin.json` and its TypeScript entrypoint; Termy validates the source, copies it into the managed plugin directory, and shows its name, version, and enabled state. The same screen can refresh the inventory, open the plugin directory, enable or disable a plugin, and uninstall it with confirmation.

The CLI can scaffold a plugin locally, install its managed copy, or install trusted source directly from GitHub:

```sh
termy plugin init my-plugin
termy plugin dev ./my-plugin
termy plugin add ./my-plugin
termy plugin add https://github.com/example/termy-plugins --path my-plugin
termy plugin status my-plugin
termy plugin disable my-plugin
termy plugin enable my-plugin
termy plugin update my-plugin
termy plugin uninstall my-plugin
```

`add` also has the `install` alias, while `remove` has the `uninstall` alias. A local directory is validated and copied into Termy's managed global plugins directory, leaving the development source untouched. Repository URLs, `/tree/<ref>/<path>` URLs, `--ref`, and `--path` are also supported. Termy resolves the selected ref to a full commit, downloads only regular files, validates the manifest and source limits, and saves the repository, requested ref, plugin subdirectory, and pinned revision for later status and updates. A repository containing multiple valid plugins requires `--path` so Termy never guesses which one to install.

`termy plugin dev ./my-plugin` installs or updates the managed local copy, then watches the development directory recursively. Source changes are debounced, source-tree validated, and swapped into the managed directory atomically. A validation or copy failure leaves the managed copy untouched; Bun load errors appear in Termy when the command palette refreshes. Pressing Ctrl+C stops watching but leaves the plugin installed. Dev mode ignores `.git` and `node_modules`, preserves enabled state and plugin storage, and refuses to overwrite a GitHub-tracked installation.

GitHub installation never clones the repository, runs package scripts, installs dependencies, or evaluates source during installation. Plugins are still trusted code when the command palette loads them through Bun, so the CLI shows a warning and requires confirmation; automation must pass `--yes` explicitly.

You can also manage the directory manually:

Create one directory per plugin under Termy's config directory:

```text
termy/
└── plugins/
    ├── termy.d.ts
    ├── .termy-cache/
    │   ├── bundles/
    │   │   └── git-tools/
    │   │       └── <content-hash>.mjs
    │   └── data/
    │       └── git-tools/
    ├── .termy-data/
    │   └── git-tools/
    │       ├── storage.json
    │       └── files/
    └── git-tools/
        ├── plugin.json
        └── plugin.ts
```

The config directory is `$XDG_CONFIG_HOME/termy` when `XDG_CONFIG_HOME` is set, `~/.config/termy` otherwise on macOS and Linux, and `%APPDATA%\termy` on Windows.

Termy manages `plugins/termy.d.ts`, `plugins/.termy-cache`, and `plugins/.termy-data`; do not edit them directly. The declarations provide the global `definePlugin` function and ambient `TermyPlugin` types for editor completion, so plugin source does not import an SDK package.

## Manifest

Every plugin directory contains a `plugin.json` manifest:

```json
{
  "$schema": "https://termy.sh/schemas/plugin.schema.json",
  "apiVersion": 1,
  "id": "hello",
  "name": "Hello",
  "version": "1.0.0",
  "capabilities": []
}
```

| Field | Required | Description |
| --- | --- | --- |
| `$schema` | no | Public [JSON Schema](https://termy.sh/schemas/plugin.schema.json) for editor validation and completion. |
| `apiVersion` | yes | Plugin API version; v1 requires `1`. |
| `id` | yes | Stable plugin ID. It should match the directory name. |
| `name` | yes | Human-readable plugin name shown by Termy. |
| `version` | no | Version shown in Settings when the plugin is managed there. |
| `main` | no | TypeScript entrypoint relative to the plugin directory. Defaults to `plugin.ts`. |
| `capabilities` | no | Termy host APIs the plugin opts into. Supports `storage` and `native-ui`; omitted means `[]`. |

Use lowercase letters, numbers, and hyphens for IDs so saved references remain stable when a display name changes. `main` must resolve to a regular file inside the plugin directory; out-of-root paths and symlinks are rejected.

`storage` enables `context.storage` and `context.paths`. `native-ui` enables
declared views and `view.open`. Unknown or duplicate values are rejected. These
declarations gate Termy APIs; they do not sandbox the trusted Bun process or its
normal file, network, and process access.

## Minimal plugin

`plugin.json`:

```json
{
  "$schema": "https://termy.sh/schemas/plugin.schema.json",
  "apiVersion": 1,
  "id": "hello",
  "name": "Hello",
  "capabilities": []
}
```

`plugin.ts`:

```ts
export default definePlugin({
  commands: [
    {
      id: "say-hello",
      title: "Hello: Greet me",
      keywords: ["hello", "example"],
      icon: "info",
      run({ context }) {
        context.toasts.success("Hello from Termy");
      },
    },
  ],
} satisfies TermyPlugin);
```

The manifest owns API and identity metadata. `definePlugin` owns runtime behavior
and contains commands plus optional user settings; do not duplicate `$schema`,
`apiVersion`, `id`, `name`, `version`, `main`, or `capabilities` in `plugin.ts`.

## Command fields

Each command accepts these fields:

| Field | Required | Description |
| --- | --- | --- |
| `id` | yes | Stable command ID, unique within the plugin. |
| `title` | yes | Label shown in the command palette. |
| `placements` | no | Surfaces that list the command: `commandPalette`, `terminalContextMenu`, and `tabContextMenu`. Defaults to `["commandPalette"]`. |
| `keywords` | no | Extra strings matched by palette search. |
| `status` | no | Compact status text shown on the palette row. |
| `enabled` | no | Set to `false` to keep the command visible but unavailable. |
| `disabledReason` | no | Explanation shown for a disabled command. |
| `icon` | no | One of `command`, `play`, `terminal`, `folder`, `link`, `clipboard`, `settings`, or `info`. |
| `inputs` | no | Text, select, and confirm prompts collected before `run`. |
| `run` | yes | Async or synchronous command handler. |

Termy namespaces runtime commands as `<plugin-id>.<command-id>`, so command IDs only need to be unique inside their plugin.

Context-menu placements are currently rendered on Linux and Windows. Commands
with inputs open the same palette input flow from any surface. A command with an
empty `placements` array stays available to keybindings without appearing in a
menu.

## Imports and build cache

Plugin source can use local relative imports inside its plugin directory:

```ts
import { formatMessage } from "./messages.ts";
```

V1 accepts local relative imports plus Bun and Node built-ins such as `bun` and `node:fs`. Every local import must resolve to a regular file inside the plugin directory. Package imports, absolute or out-of-root source paths, and symlinks are rejected.

Termy does not run `bun install` or automatically install missing packages. Keep the complete local source tree inside the plugin directory.

When the command palette opens, Termy fingerprints each manifest and plugin source tree. A new content hash is bundled once with `Bun.build({ target: "bun" })` and written to:

```text
plugins/.termy-cache/bundles/<plugin-id>/<content-hash>.mjs
```

The bundle includes local relative TypeScript imports. An unchanged plugin reuses its cached bundle; a changed manifest or source tree produces a new hash and bundle, then Termy imports it in that plugin's warm Worker.

Termy deliberately does not use `bun build --compile`. A compiled plugin executable embeds the Bun runtime in every plugin artifact, multiplying disk and startup overhead. Cached `.mjs` bundles share the one persistent Bun host while still avoiding repeated transpilation.

## Inputs

Inputs appear sequentially in their array order. Once Termy has collected and validated every value, it calls `run({ inputs, context })`. `inputs` is keyed by input ID and contains strings or booleans.

```ts
inputs: [
  {
    id: "label",
    type: "text",
    label: "Label",
    placeholder: "Release name",
    defaultValue: "next",
    required: true,
    maxLength: 80,
  },
  {
    id: "target",
    type: "select",
    label: "Target",
    required: true,
    options: [
      { value: "debug", label: "Debug", keywords: ["dev"] },
      { value: "release", label: "Release", keywords: ["production"] },
    ],
  },
  {
    id: "confirmed",
    type: "confirm",
    label: "Run now?",
    defaultValue: true,
  },
]
```

Text inputs support `placeholder`, `defaultValue`, `required`, and `maxLength`. Select inputs support `placeholder`, `defaultValue`, `required`, and a fixed `options` array. Confirm inputs return a boolean and support `defaultValue`.

Treat text input as untrusted. Do not interpolate it directly into a shell command; prefer a select input mapped to fixed commands, or apply quoting appropriate for the target shell.

## Plugin settings

Declare user-editable settings beside `commands`. Termy validates the schema, renders it under **Settings → Plugins**, and infers the value type for `context.settings.get(...)`:

```ts
export default definePlugin({
  settings: {
    autoFetch: {
      type: "toggle",
      title: "Fetch before running",
      defaultValue: true,
    },
    format: {
      type: "select",
      title: "Output format",
      options: [
        { value: "compact", label: "Compact" },
        { value: "detailed", label: "Detailed" },
      ],
      defaultValue: "compact",
    },
    username: {
      type: "text",
      title: "Username",
      placeholder: "octocat",
      maxLength: 100,
    },
    token: {
      type: "secret",
      title: "API token",
      description: "Used for authenticated requests.",
    },
  },
  commands: [{
    id: "run",
    title: "Example: Run",
    run({ context }) {
      const format = context.settings.get("format");
      const token = context.settings.get("token");
      context.toasts.info(`Running in ${format} mode${token ? " with auth" : ""}`);
    },
  }],
});
```

Supported types are `toggle`, `text`, `select`, and `secret`. Text and secret settings accept `placeholder` and `maxLength`; toggles accept a boolean default; selects require fixed options and may choose a default. Ordinary overrides live in the plugin's managed data directory. Secret values are masked in the UI and stored through the operating-system credential store, never in `settings.json`. Changes apply to the next command invocation without restarting the Worker, and uninstalling the plugin removes its settings and secrets.

## Context

Every handler receives the current Termy context:

```ts
type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

type PluginContext = {
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
  readonly settings: {
    get<T = string | boolean>(key: string): T | undefined;
  };
  readonly toasts: {
    info(message: string): void;
    success(message: string): void;
    warning(message: string): void;
    error(message: string): void;
  };
  readonly storage: {
    get<T = JsonValue>(key: string): Promise<T | undefined>;
    set(key: string, value: JsonValue): Promise<void>;
    delete(key: string): Promise<boolean>;
    clear(): Promise<void>;
  };
  readonly paths: {
    readonly dataDirectory: string;
    readonly cacheDirectory: string;
  };
};
```

The context is a read-only snapshot taken when the command starts. `shell` is the resolved shell program Termy uses to launch sessions, while `runtime` identifies the active `native` or `tmux` backend. `workingDirectory`, `activeCommand`, `selectedText`, `activeTab`, and `activePane` are absent when Termy cannot provide them. Tab and pane indexes are zero-based. Selected text is capped at 64 KiB on a UTF-8 boundary; check `selectedTextTruncated` before assuming it is complete.

For example, a command can copy the current terminal selection while identifying where it came from:

```ts
run({ context }) {
  if (!context.selectedText) {
    context.toasts.info("Select terminal text first");
    return;
  }
  const tab = context.activeTab ? `Tab ${context.activeTab.index + 1}` : "Termy";
  return {
    type: "clipboard.write",
    text: `${tab} (${context.runtime})\n\n${context.selectedText}`,
  };
}
```

Use `context.toasts` when a command only needs to notify the user; no SDK import or returned action is required:

```ts
run({ context }) {
  context.toasts.success("Finished syncing");
}
```

Declare `"storage"` in `plugin.json` before using `context.storage` or
`context.paths`. `context.storage` persists small JSON values outside the
content-hashed plugin source. Storage is isolated by plugin, limited to 512 keys
and 1 MiB, survives reloads and updates, and is deleted when the plugin is
uninstalled:

```ts
async run({ context }) {
  const runs = (await context.storage.get<number>("runs")) ?? 0;
  await context.storage.set("runs", runs + 1);
  context.toasts.success(`Run ${runs + 1}`);
}
```

Use `context.paths.dataDirectory` for larger persistent files and `context.paths.cacheDirectory` for disposable files with Bun or Node filesystem APIs. Both directories are outside the plugin source tree, so writing there does not trigger a rebuild. Storage is plain local JSON; declare a `secret` plugin setting for tokens and passwords so Termy uses the operating-system credential store.

## Native JSX views

Plugins can open small native tools written as TSX. JSX is only authoring sugar: Bun lowers it through the frozen `TermyUI` runtime, the plugin Worker converts it to a bounded document tree, Rust validates that tree again, and Termy renders the allowlisted nodes with GPUI. Plugins never receive GPUI objects and cannot supply HTML, CSS, callbacks, colors, fonts, asset paths, or arbitrary native properties.

Set the manifest entrypoint to a `.tsx` file:

```json
{
  "$schema": "https://termy.sh/schemas/plugin.schema.json",
  "apiVersion": 1,
  "id": "todos",
  "name": "Todos",
  "main": "plugin.tsx",
  "capabilities": ["storage", "native-ui"]
}
```

Declare views beside `commands`, then open one with a `view.open` action:

```tsx
/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */

export default definePlugin({
  commands: [{
    id: "open",
    title: "Todos: Open",
    run() {
      return { type: "view.open", view: "todos" };
    },
  }],

  views: {
    todos: {
      title: "Todos",

      async render({ context }) {
        const todos = await context.storage.get<Array<{
          id: string;
          title: string;
          done: boolean;
        }>>("todos") ?? [];

        return (
          <TermyUI.Column gap="medium">
            <TermyUI.Row gap="small" align="center">
              <TermyUI.TextInput
                id="title"
                placeholder="Add a task…"
                submit="add"
              />
              <TermyUI.Button id="add-button" action="add" variant="primary">
                Add
              </TermyUI.Button>
            </TermyUI.Row>
            <TermyUI.Divider />
            {todos.slice(0, 24).map((todo) => (
              <TermyUI.Checkbox
                id={`todo-${todo.id}`}
                action="toggle"
                payload={todo.id}
                checked={todo.done}
              >
                {todo.title}
              </TermyUI.Checkbox>
            ))}
          </TermyUI.Column>
        );
      },

      async onAction({ action, values, context }) {
        // Update storage from action.id, action.payload, action.value, and values.
        // Termy calls render() again after this handler finishes.
      },
    },
  },
} satisfies TermyPlugin);
```

`view.open` uses a centered modal by default. To render the same bounded native
view inside the command palette's results area, while keeping its search and
footer, set `target: "commandPalette"`:

```ts
return { type: "view.open", view: "todos", target: "commandPalette" };
```

The v1 component set is `TermyUI.Column`, `Row`, `Text`, `TextInput`, `Button`, `Checkbox`, `Divider`, `Spacer`, and fragments. Layout uses fixed gap and alignment enums; text uses fixed variant and tone enums; buttons use `secondary`, `primary`, or `danger`. Every input and interactive control needs a unique lowercase stable `id`. Controls send a named `action`, optional string `payload`, their current value, and the current text/checkbox `values` map to `onAction`. No JavaScript callback crosses into the native renderer.

Views are limited to 32 per plugin, 256 nodes, 16 levels of nesting, 64 children per node, 64 value-bearing controls, and 4,096 characters per text value. Termy serializes interactions per plugin, disables controls while one is running, rejects stale plugin revisions, and rerenders after successful actions. Escape, the close button, or clicking outside the panel closes a view.

Paginate or window dynamic lists so a large saved collection stays inside the document and value-control limits. The complete [Todo example](../examples/plugins/todos/plugin.tsx) includes pagination.

## Lifecycle events

Subscribe by adding an `events` object beside `commands`. Termy only dispatches events a plugin explicitly handles; an event-only plugin should use `commands: []`.

```ts
export default definePlugin({
  commands: [],
  events: {
    "terminal.ready"({ context }) {
      context.toasts.info("Terminal ready");
    },
    "tab.activated"({ event, context }) {
      context.toasts.info(`Active tab: ${context.activeTab?.index ?? "unknown"}`);
    },
    "workingDirectory.changed"({ event }) {
      console.log(event.previousWorkingDirectory, "->", event.workingDirectory);
    },
    "command.finished"({ event }) {
      console.log(event.command, event.exitCode, event.durationMs);
    },
  },
} satisfies TermyPlugin);
```

| Event | Payload | When it runs |
| --- | --- | --- |
| `terminal.ready` | `{ type }` | Once for a terminal window after the plugin catalog is ready. |
| `tab.activated` | `{ type, previousTabIndex? }` | When the active terminal tab changes. |
| `workingDirectory.changed` | `{ type, previousWorkingDirectory?, workingDirectory? }` | When the active terminal's working directory changes. |
| `command.finished` | `{ type, command?, exitCode?, durationMs? }` | When a command finishes in the active terminal. |

Event handlers receive the same read-only context, settings, storage, paths, and toast helpers as commands. They may be synchronous or async and return the same native actions. Events stay ordered within each plugin; different subscribed plugins can run concurrently and retain the normal execution timeout.

Native shell integration supplies the exit code and measured duration for `command.finished`. Tmux completion is inferred from command-state changes, so `exitCode` and other unavailable fields are omitted instead of guessed. Lifecycle events describe the active terminal only; background tabs do not emit active-context changes.

## Actions

A handler can return nothing, one action, an action array, or `{ actions: [...] }`. Async handlers may return the same values through a Promise.

| Action | Shape | Effect |
| --- | --- | --- |
| Run in the terminal | `{ type: "terminal.run", command, workingDirectory? }` | Run a shell command, optionally in a specific directory. |
| Run a Termy command | `{ type: "termy.command", command }` | Invoke a built-in Termy command by its stable command name. |
| Copy text | `{ type: "clipboard.write", text }` | Write text to the system clipboard. |
| Open a URL | `{ type: "url.open", url }` | Open an `http` or `https` URL with the system browser. |
| Show a toast | `{ type: "toast", level, message }` | Show an `info`, `success`, `warning`, or `error` notification. |

Toasts emitted through `context.toasts` run before returned actions, and returned actions keep their existing order. Keep handlers focused and return only the effects the command needs.

## Reloading and failures

Termy checks plugin content when the command palette opens. When `plugin.json`, `plugin.ts`, or another local source file changes, Termy creates or reuses the matching bundle, replaces that plugin's Worker, and refreshes the command list; restarting Termy is unnecessary.

## Plugin keybindings

Bind a plugin command in `~/.config/termy/config.txt` with its manifest IDs:

```txt
keybind = secondary-g=plugin:git-tools/status
```

Commands without inputs run immediately. Commands with inputs open their input form. Termy refreshes the plugin catalog before invoking the shortcut, so saved plugin changes are picked up. Normal keybinding ordering applies: later lines win, `unbind` removes the shortcut, and a task keybinding takes priority on conflicts.

Disabling a plugin in Settings keeps its files and storage installed but removes its commands the next time the command palette refreshes. Enabling it makes the commands available again. Uninstalling removes Termy's managed copy, storage, and cache, but not the source folder you originally selected.

Termy rejects actions and native-view updates returned by a request that was
already running when its plugin changed, was disabled, or was removed.

Plugin loading and command execution have timeouts. A thrown error or timeout is contained to that plugin Worker and reported in Termy, while other plugins and the terminal keep running. A subprocess started by plugin code can outlive its Worker, so plugins that spawn processes must stop them themselves when cancellation matters. Plugins share the persistent host transport; if that transport exits, Termy discards it and rebuilds the host and Workers on the next plugin refresh instead of taking down the app. Fix a failed plugin and reopen the palette to load it again.

## Security and v1 limits

Plugins are trusted local code. Worker isolation, import validation, and timeouts improve reliability, but they are not a security sandbox: after loading, a plugin runs through Bun with your user account's access to files, network, and processes. Only install plugins whose source you trust.

The v1 runtime supports command-palette commands, native JSX views, lifecycle events, and user-configured keybindings. It does not provide arbitrary native UI access, build hooks, package imports, or automatic package installation. Bun is launched with dependency installation and environment-file loading disabled; local relative TypeScript imports are bundled from the plugin directory, while Bun and Node built-ins remain available at runtime.

See [`examples/plugins/git-tools/plugin.json`](../examples/plugins/git-tools/plugin.json) and [`plugin.ts`](../examples/plugins/git-tools/plugin.ts) for a safe command example that maps a select value to a fixed shell command.

See [`examples/plugins/todos/plugin.json`](../examples/plugins/todos/plugin.json) and [`plugin.tsx`](../examples/plugins/todos/plugin.tsx) for a persistent native JSX view with add, toggle, and delete actions.
