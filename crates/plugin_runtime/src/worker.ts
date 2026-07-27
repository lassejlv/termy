// One isolated Bun Worker per loaded Termy plugin.
import { createHash } from "node:crypto";
import {
  mkdir,
  readFile,
  readdir,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { isBuiltin } from "node:module";
import {
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
} from "node:path";
import { pathToFileURL } from "node:url";

type PluginSource = {
  id: string;
  root: string;
  cacheKey: string;
};
const PLUGIN_CAPABILITIES = ["storage", "native-ui"] as const;
type PluginCapability = (typeof PLUGIN_CAPABILITIES)[number];
type PreparedPluginSource = PluginSource & {
  name: string;
  version?: string;
  path: string;
  capabilities: PluginCapability[];
};
type PluginManifest = {
  apiVersion: number;
  id: string;
  name: string;
  version?: string;
  main?: string;
  capabilities?: unknown;
};
type CapturedFile = { relativePath: string; contents: Uint8Array };
type PluginToastLevel = "info" | "success" | "warning" | "error";
type PluginToasts = {
  info: (message: string) => void;
  success: (message: string) => void;
  warning: (message: string) => void;
  error: (message: string) => void;
};
type PluginJsonValue =
  | null
  | boolean
  | number
  | string
  | PluginJsonValue[]
  | { [key: string]: PluginJsonValue };
type PluginStorage = {
  get: <T = PluginJsonValue>(key: string) => Promise<T | undefined>;
  set: (key: string, value: PluginJsonValue) => Promise<void>;
  delete: (key: string) => Promise<boolean>;
  clear: () => Promise<void>;
};
type PluginPaths = {
  dataDirectory: string;
  cacheDirectory: string;
};
type PluginSettings = {
  get: <T = string | boolean>(key: string) => T | undefined;
};
type PluginServices = { storage: PluginStorage; paths: PluginPaths };
type PluginContext = Record<string, unknown> &
  PluginServices & { settings: PluginSettings; toasts: PluginToasts };
type PluginCommand = {
  id: string;
  title: string;
  placements?: string[];
  keywords?: string[];
  status?: string;
  enabled?: boolean;
  disabledReason?: string;
  icon?: string;
  inputs?: unknown[];
  timeoutMs?: number;
  run: (request: {
    inputs: Record<string, unknown>;
    context: PluginContext;
  }) => unknown;
};
type PluginEventName =
  | "terminal.ready"
  | "tab.activated"
  | "workingDirectory.changed"
  | "command.finished";
type PluginEventHandler = (request: {
  event: Record<string, unknown>;
  context: PluginContext;
}) => unknown;
type PluginSettingDefinition = {
  type: "toggle" | "text" | "select" | "secret";
  title: string;
  description?: string;
  placeholder?: string;
  defaultValue?: string | boolean;
  maxLength?: number;
  options?: Array<{ value: string; label: string }>;
};
type PluginViewValue = string | boolean;
type PluginViewAction = {
  id: string;
  controlId: string;
  payload?: string;
  value?: PluginViewValue;
};
type PluginViewDefinition = {
  title: string;
  timeoutMs?: number;
  render: (request: { context: PluginContext }) => unknown;
  onAction?: (request: {
    action: PluginViewAction;
    values: Record<string, PluginViewValue>;
    context: PluginContext;
  }) => unknown;
};
type PluginDefinition = {
  commands: PluginCommand[];
  events?: Partial<Record<PluginEventName, PluginEventHandler>>;
  views?: Record<string, PluginViewDefinition>;
  settings?: Record<string, PluginSettingDefinition>;
};

type TermyUiComponent = (props: Record<string, unknown>) => unknown;
type TermyUiRuntime = {
  createElement: (
    component: TermyUiComponent | symbol,
    props: Record<string, unknown> | null,
    ...children: unknown[]
  ) => unknown;
  Fragment: symbol;
  Column: TermyUiComponent;
  Row: TermyUiComponent;
  Text: TermyUiComponent;
  TextInput: TermyUiComponent;
  Button: TermyUiComponent;
  Checkbox: TermyUiComponent;
  Divider: TermyUiComponent;
  Spacer: TermyUiComponent;
};

declare global {
  var definePlugin: <T extends PluginDefinition>(plugin: T) => T;
  var TermyUI: TermyUiRuntime;
}

const MAX_PLUGIN_TREE_BYTES = 16 * 1024 * 1024;
const MAX_PLUGIN_TREE_FILES = 4_096;
const MAX_STORAGE_BYTES = 1024 * 1024;
const MAX_STORAGE_ENTRIES = 512;
const MAX_PLUGIN_VIEWS = 32;
const MAX_VIEW_NODES = 256;
const MAX_VIEW_DEPTH = 16;
const MAX_VIEW_CHILDREN = 64;
const MAX_VIEW_VALUES = 64;
globalThis.definePlugin = (plugin) => plugin;
let plugin: PluginDefinition | undefined;
let pluginServices: PluginServices | undefined;
let commandHandlers = new Map<string, PluginCommand["run"]>();
let eventHandlers = new Map<PluginEventName, PluginEventHandler>();
let viewHandlers = new Map<string, PluginViewDefinition>();
let pluginCapabilities = new Set<PluginCapability>();
let queue = Promise.resolve();

// Keep ordinary plugin output on Termy's diagnostic stream.
process.stdout.write = process.stderr.write.bind(process.stderr) as typeof process.stdout.write;

function errorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (
    error &&
    typeof error === "object" &&
    Array.isArray((error as { logs?: unknown }).logs)
  ) {
    const detail = (error as { logs: Array<{ message?: unknown }> }).logs
      .map((log) => String(log.message || log))
      .join("; ");
    if (detail) return `${message}: ${detail}`;
  }
  if (error instanceof Error && error.stack && message === "Bundle failed") {
    return error.stack;
  }
  return message;
}

function logValue(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function requireCapability(capability: PluginCapability, feature: string): void {
  if (!pluginCapabilities.has(capability)) {
    throw new Error(
      `${feature} requires capability \`${capability}\` in plugin.json`,
    );
  }
}

function textLength(value: string): number {
  return Array.from(value).length;
}

for (const level of ["log", "info", "warn", "error"] as const) {
  console[level] = (...values: unknown[]) => {
    const pluginId = process.env.TERMY_PLUGIN_ID || "unknown";
    process.stderr.write(
      `[termy plugin ${pluginId}] ${values.map(logValue).join(" ")}\n`,
    );
  };
}

function assertId(value: unknown, label: string): asserts value is string {
  if (
    typeof value !== "string" ||
    !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(value)
  ) {
    throw new Error(`${label} must be a lowercase stable ID`);
  }
}

function assertText(value: unknown, label: string, max = 300): asserts value is string {
  if (typeof value !== "string" || value.trim() === "" || textLength(value) > max) {
    throw new Error(`${label} must be a non-empty string up to ${max} characters`);
  }
}

function optionalText(value: unknown, label: string, max: number): string | undefined {
  if (value === undefined) return undefined;
  assertText(value, label, max);
  return value;
}

function optionalString(value: unknown, label: string, max: number): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || textLength(value) > max) {
    throw new Error(`${label} must be a string up to ${max} characters`);
  }
  return value;
}

function normalizeKeywords(value: unknown, label: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 64) {
    throw new Error(`${label} must contain at most 64 strings`);
  }
  return value.map((keyword) => {
    assertText(keyword, label, 200);
    return keyword;
  });
}

function flattenUiChildren(value: unknown, output: unknown[] = []): unknown[] {
  if (Array.isArray(value)) {
    for (const child of value) flattenUiChildren(child, output);
  } else if (value !== undefined && value !== null && value !== false && value !== true) {
    output.push(value);
  }
  return output;
}

function componentProps(
  name: string,
  value: Record<string, unknown>,
  allowed: readonly string[],
  allowsChildren = true,
): Record<string, unknown> {
  const allowedSet = new Set(["key", ...allowed]);
  for (const key of Object.keys(value)) {
    if (key === "children") {
      if (allowsChildren || flattenUiChildren(value.children).length === 0) continue;
      throw new Error(`${name} does not support children`);
    }
    if (!allowedSet.has(key)) throw new Error(`${name} does not support prop ${key}`);
  }
  return value;
}

function componentText(value: unknown, label: string): string {
  const parts = flattenUiChildren(value).map((child) => {
    if (typeof child !== "string" && typeof child !== "number") {
      throw new Error(`${label} children must be text`);
    }
    return String(child);
  });
  const text = parts.join("");
  assertText(text, label, 4_096);
  return text;
}

function uiContainer(type: "column" | "row", props: Record<string, unknown>): unknown {
  componentProps(type, props, ["gap", "align"]);
  return {
    type,
    gap: props.gap ?? "medium",
    align: props.align ?? "start",
    children: flattenUiChildren(props.children),
  };
}

const Fragment = Symbol("TermyUI.Fragment");
const TermyUiRuntime: TermyUiRuntime = Object.freeze({
  createElement(component, props, ...children) {
    const nextProps = { ...(props || {}), children };
    if (component === Fragment) return flattenUiChildren(children);
    if (typeof component !== "function") {
      throw new Error("Termy UI only supports TermyUI components");
    }
    return component(nextProps);
  },
  Fragment,
  Column: (props) => uiContainer("column", props),
  Row: (props) => uiContainer("row", props),
  Text: (props) => {
    componentProps("Text", props, ["variant", "tone"]);
    return {
      type: "text",
      text: componentText(props.children, "Text"),
      variant: props.variant ?? "body",
      tone: props.tone ?? "default",
    };
  },
  TextInput: (props) => {
    componentProps("TextInput", props, [
      "id",
      "label",
      "placeholder",
      "value",
      "maxLength",
      "submit",
      "disabled",
    ], false);
    return { type: "textInput", ...props, children: undefined, key: undefined };
  },
  Button: (props) => {
    componentProps("Button", props, [
      "id",
      "action",
      "payload",
      "variant",
      "disabled",
    ]);
    return {
      type: "button",
      ...props,
      children: undefined,
      key: undefined,
      label: componentText(props.children, "Button"),
    };
  },
  Checkbox: (props) => {
    componentProps("Checkbox", props, [
      "id",
      "action",
      "payload",
      "checked",
      "disabled",
    ]);
    return {
      type: "checkbox",
      ...props,
      children: undefined,
      key: undefined,
      label: componentText(props.children, "Checkbox"),
    };
  },
  Divider: (props) => {
    componentProps("Divider", props, [], false);
    return { type: "divider" };
  },
  Spacer: (props) => {
    componentProps("Spacer", props, ["size"], false);
    return { type: "spacer", size: props.size ?? "medium" };
  },
});
globalThis.TermyUI = TermyUiRuntime;

const UI_GAPS = ["none", "small", "medium", "large"] as const;
const UI_ALIGNMENTS = ["start", "center", "end", "stretch"] as const;
const UI_TEXT_VARIANTS = ["heading", "body", "caption", "code"] as const;
const UI_TONES = ["default", "muted", "success", "danger"] as const;
const UI_BUTTON_VARIANTS = ["secondary", "primary", "danger"] as const;

function enumValue(
  value: unknown,
  allowed: readonly string[],
  label: string,
  fallback: string,
): string {
  const normalized = value ?? fallback;
  if (typeof normalized !== "string" || !allowed.includes(normalized)) {
    throw new Error(`${label} has an unsupported value`);
  }
  return normalized;
}

function booleanValue(value: unknown, label: string, fallback = false): boolean {
  if (value === undefined) return fallback;
  if (typeof value !== "boolean") throw new Error(`${label} must be a boolean`);
  return value;
}

function normalizeViewDocument(value: unknown): Record<string, unknown>[] {
  let count = 0;
  let valueCount = 0;
  const controlIds = new Set<string>();

  const nodeKeys = (
    node: Record<string, unknown>,
    allowed: readonly string[],
    label: string,
  ): void => {
    const allowedSet = new Set(["type", ...allowed]);
    for (const key of Object.keys(node)) {
      if (!allowedSet.has(key) && node[key] !== undefined) {
        throw new Error(`${label} does not support prop ${key}`);
      }
    }
  };

  const normalizeChildren = (candidate: unknown, depth: number): Record<string, unknown>[] => {
    const children = flattenUiChildren(candidate);
    if (children.length > MAX_VIEW_CHILDREN) {
      throw new Error(`Termy UI nodes may have at most ${MAX_VIEW_CHILDREN} children`);
    }
    return children.map((child) => normalizeNode(child, depth));
  };

  const controlId = (candidate: unknown, label: string): string => {
    assertId(candidate, label);
    if (controlIds.has(candidate)) throw new Error(`Duplicate Termy UI control ID ${candidate}`);
    controlIds.add(candidate);
    return candidate;
  };

  const normalizeNode = (candidate: unknown, depth: number): Record<string, unknown> => {
    if (depth > MAX_VIEW_DEPTH) {
      throw new Error(`Termy UI exceeds the maximum depth of ${MAX_VIEW_DEPTH}`);
    }
    count += 1;
    if (count > MAX_VIEW_NODES) {
      throw new Error(`Termy UI exceeds the maximum of ${MAX_VIEW_NODES} nodes`);
    }
    if (typeof candidate === "string" || typeof candidate === "number") {
      const text = String(candidate);
      assertText(text, "Termy UI text", 4_096);
      return { type: "text", text, variant: "body", tone: "default" };
    }
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
      throw new Error("Termy UI render returned an unsupported child");
    }
    const node = candidate as Record<string, unknown>;
    if (node.type === "column" || node.type === "row") {
      nodeKeys(node, ["gap", "align", "children"], String(node.type));
      return {
        type: node.type,
        gap: enumValue(node.gap, UI_GAPS, `${node.type} gap`, "medium"),
        align: enumValue(node.align, UI_ALIGNMENTS, `${node.type} alignment`, "start"),
        children: normalizeChildren(node.children, depth + 1),
      };
    }
    if (node.type === "text") {
      nodeKeys(node, ["text", "variant", "tone"], "Text");
      assertText(node.text, "Termy UI text", 4_096);
      return {
        type: "text",
        text: node.text,
        variant: enumValue(node.variant, UI_TEXT_VARIANTS, "Text variant", "body"),
        tone: enumValue(node.tone, UI_TONES, "Text tone", "default"),
      };
    }
    if (node.type === "textInput") {
      nodeKeys(
        node,
        ["id", "label", "placeholder", "value", "maxLength", "submit", "disabled"],
        "TextInput",
      );
      const id = controlId(node.id, "TextInput ID");
      valueCount += 1;
      if (valueCount > MAX_VIEW_VALUES) {
        throw new Error(`Termy UI may contain at most ${MAX_VIEW_VALUES} value controls`);
      }
      const maxLength = node.maxLength === undefined ? 1_024 : Number(node.maxLength);
      if (!Number.isInteger(maxLength) || maxLength < 1 || maxLength > 4_096) {
        throw new Error(`TextInput ${id} maxLength must be between 1 and 4096`);
      }
      const value = optionalString(node.value, `TextInput ${id} value`, maxLength) ?? "";
      if (node.submit !== undefined) assertId(node.submit, `TextInput ${id} submit action`);
      return {
        type: "textInput",
        id,
        label: optionalText(node.label, `TextInput ${id} label`, 200),
        placeholder: optionalText(node.placeholder, `TextInput ${id} placeholder`, 300),
        value,
        maxLength,
        submit: node.submit,
        disabled: booleanValue(node.disabled, `TextInput ${id} disabled`),
      };
    }
    if (node.type === "button" || node.type === "checkbox") {
      nodeKeys(
        node,
        node.type === "button"
          ? ["id", "action", "label", "payload", "variant", "disabled"]
          : ["id", "action", "label", "payload", "checked", "disabled"],
        String(node.type),
      );
      const id = controlId(node.id, `${node.type} ID`);
      assertId(node.action, `${node.type} ${id} action`);
      assertText(node.label, `${node.type} ${id} label`, 300);
      const common = {
        type: node.type,
        id,
        action: node.action,
        label: node.label,
        payload: optionalText(node.payload, `${node.type} ${id} payload`, 1_024),
        disabled: booleanValue(node.disabled, `${node.type} ${id} disabled`),
      };
      if (node.type === "button") {
        return {
          ...common,
          variant: enumValue(
            node.variant,
            UI_BUTTON_VARIANTS,
            `Button ${id} variant`,
            "secondary",
          ),
        };
      }
      valueCount += 1;
      if (valueCount > MAX_VIEW_VALUES) {
        throw new Error(`Termy UI may contain at most ${MAX_VIEW_VALUES} value controls`);
      }
      return {
        ...common,
        checked: booleanValue(node.checked, `Checkbox ${id} checked`),
      };
    }
    if (node.type === "divider") {
      nodeKeys(node, [], "Divider");
      return { type: "divider" };
    }
    if (node.type === "spacer") {
      nodeKeys(node, ["size"], "Spacer");
      return {
        type: "spacer",
        size: enumValue(node.size, UI_GAPS, "Spacer size", "medium"),
      };
    }
    throw new Error(`Unsupported Termy UI node ${String(node.type)}`);
  };

  return normalizeChildren(value, 1);
}

async function capturePlugin(source: PluginSource): Promise<CapturedFile[]> {
  const root = resolve(source.root);
  const files: CapturedFile[] = [];
  let totalBytes = 0;

  const visit = async (directory: string, prefix: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) =>
      Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)),
    );
    for (const entry of entries) {
      if (
        entry.name === ".git" ||
        entry.name === "node_modules" ||
        entry.name === ".termy-disabled" ||
        entry.name === ".termy-source.json"
      ) {
        continue;
      }
      const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) {
        throw new Error(`Plugin source cannot contain symlink ${relativePath}`);
      }
      if (entry.isDirectory()) {
        await visit(path, relativePath);
        continue;
      }
      if (!entry.isFile()) {
        throw new Error(`Plugin source contains unsupported file ${relativePath}`);
      }
      const contents = new Uint8Array(await readFile(path));
      totalBytes += contents.byteLength;
      if (totalBytes > MAX_PLUGIN_TREE_BYTES) {
        throw new Error("Plugin source tree exceeds 16 MiB");
      }
      files.push({ relativePath, contents });
      if (files.length > MAX_PLUGIN_TREE_FILES) {
        throw new Error(`Plugin source tree exceeds ${MAX_PLUGIN_TREE_FILES} files`);
      }
    }
  };

  await visit(root, "");
  files.sort((left, right) =>
    Buffer.compare(
      Buffer.from(left.relativePath),
      Buffer.from(right.relativePath),
    ),
  );
  const hash = createHash("sha256");
  hash.update("termy-plugin-bundle-v2\0");
  for (const file of files) {
    hash.update(file.relativePath);
    hash.update(new Uint8Array([0]));
    hash.update(file.contents);
    hash.update(new Uint8Array([0]));
  }
  const actualCacheKey = hash.digest("hex");
  if (actualCacheKey !== source.cacheKey) {
    throw new Error("Plugin changed while it was loading; reopen the command palette");
  }
  return files;
}

function parseManifest(source: PluginSource, files: CapturedFile[]): {
  manifest: PluginManifest;
  entrypoint: string;
  capabilities: PluginCapability[];
} {
  const manifestFile = files.find((file) => file.relativePath === "plugin.json");
  if (!manifestFile) throw new Error("plugin.json is missing");
  let manifest: PluginManifest;
  try {
    manifest = JSON.parse(Buffer.from(manifestFile.contents).toString("utf8")) as PluginManifest;
  } catch (error) {
    throw new Error(`Invalid plugin.json: ${errorMessage(error)}`);
  }
  if (!manifest || typeof manifest !== "object") {
    throw new Error("plugin.json must be an object");
  }
  if (manifest.apiVersion !== 1) throw new Error("plugin.json apiVersion must be 1");
  assertId(manifest.id, "plugin.json id");
  if (manifest.id !== source.id) {
    throw new Error(`plugin.json id ${manifest.id} must match directory ${source.id}`);
  }
  assertText(manifest.name, "plugin.json name", 200);
  if (manifest.version !== undefined) {
    assertText(manifest.version, "plugin.json version", 100);
  }
  const capabilities = normalizeCapabilities(manifest.capabilities);
  const main = manifest.main === undefined
    ? "plugin.ts"
    : optionalText(manifest.main, "plugin.json main", 1_024)!;
  if (isAbsolute(main)) throw new Error("plugin.json main must be inside the plugin directory");
  const root = resolve(source.root);
  const absoluteEntrypoint = resolve(root, main);
  const relativeEntrypoint = relative(root, absoluteEntrypoint);
  if (
    relativeEntrypoint === "" ||
    relativeEntrypoint === ".." ||
    relativeEntrypoint.startsWith(`..${sep}`) ||
    isAbsolute(relativeEntrypoint)
  ) {
    throw new Error("plugin.json main must be a file inside the plugin directory");
  }
  const entrypoint = relativeEntrypoint.split(sep).join("/");
  if (!files.some((file) => file.relativePath === entrypoint)) {
    throw new Error(`Plugin entrypoint does not exist: ${main}`);
  }
  return { manifest, entrypoint, capabilities };
}

function normalizeCapabilities(value: unknown): PluginCapability[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) {
    throw new Error("plugin.json capabilities must be an array");
  }
  const capabilities: PluginCapability[] = [];
  const seen = new Set<PluginCapability>();
  for (const candidate of value) {
    if (typeof candidate !== "string") {
      throw new Error("plugin.json capabilities must contain only strings");
    }
    if (!PLUGIN_CAPABILITIES.includes(candidate as PluginCapability)) {
      throw new Error(`plugin.json capability \`${candidate}\` is not supported`);
    }
    const capability = candidate as PluginCapability;
    if (seen.has(capability)) {
      throw new Error(`plugin.json capability \`${capability}\` is duplicated`);
    }
    seen.add(capability);
    capabilities.push(capability);
  }
  return capabilities;
}

function pathIsInside(root: string, candidate: string): boolean {
  const relativePath = relative(root, candidate);
  return (
    relativePath !== "" &&
    relativePath !== ".." &&
    !relativePath.startsWith(`..${sep}`) &&
    !isAbsolute(relativePath)
  );
}

async function bundlePlugin(
  source: PluginSource,
  files: CapturedFile[],
  entrypoint: string,
  bundleCacheRoot: string,
): Promise<string> {
  const bundleDirectory = join(bundleCacheRoot, source.id);
  const bundlePath = join(bundleDirectory, `${source.cacheKey}.mjs`);
  await mkdir(bundleDirectory, { recursive: true });
  if (!(await Bun.file(bundlePath).exists())) {
    const snapshotRoot = join(
      bundleDirectory,
      `.capture-${process.pid}-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    );
    try {
      for (const file of files) {
        const target = join(snapshotRoot, ...file.relativePath.split("/"));
        await mkdir(dirname(target), { recursive: true });
        await Bun.write(target, file.contents);
      }
      const snapshotEntrypoint = join(snapshotRoot, ...entrypoint.split("/"));
      let build: Awaited<ReturnType<typeof Bun.build>>;
      try {
        build = await Bun.build({
          entrypoints: [snapshotEntrypoint],
          target: "bun",
          format: "esm",
          minify: false,
          sourcemap: "inline",
          jsx: {
            runtime: "classic",
            factory: "TermyUI.createElement",
            fragment: "TermyUI.Fragment",
          },
          plugins: [
            {
              name: "termy-plugin-import-boundary",
              setup(builder) {
                builder.onResolve({ filter: /.*/ }, (args) => {
                  if (args.kind === "entry-point") return;
                  const specifier = args.path;
                  if (
                    isAbsolute(specifier) &&
                    args.importer === "" &&
                    pathIsInside(snapshotRoot, specifier)
                  ) {
                    return;
                  }
                  if (
                    isBuiltin(specifier) ||
                    specifier === "bun" ||
                    specifier.startsWith("bun:")
                  ) {
                    return;
                  }
                  if (specifier.startsWith(".")) {
                    const candidate = resolve(args.resolveDir, specifier);
                    if (pathIsInside(snapshotRoot, candidate)) return;
                    throw new Error(`Plugin import escapes its directory: ${specifier}`);
                  }
                  throw new Error(
                    `Plugin package or absolute import is not supported: ${specifier}`,
                  );
                });
              },
            },
          ],
        });
      } catch (error) {
        throw new Error(`Failed to bundle plugin ${source.id}: ${Bun.inspect(error)}`);
      }
      if (!build.success) {
        const detail = build.logs
          .map((log) => ("message" in log ? String(log.message) : String(log)))
          .join("; ");
        throw new Error(
          `Failed to bundle plugin ${source.id}: ${detail || "unknown error"}`,
        );
      }
      const output = build.outputs.find((artifact) => artifact.kind === "entry-point");
      if (!output) throw new Error(`Plugin ${source.id} produced no executable bundle`);
      const temporaryPath = `${bundlePath}.${process.pid}.${Date.now()}.tmp`;
      await Bun.write(temporaryPath, output);
      try {
        await rename(temporaryPath, bundlePath);
      } catch (error) {
        await rm(temporaryPath, { force: true });
        if (!(await Bun.file(bundlePath).exists())) throw error;
      }
    } finally {
      await rm(snapshotRoot, { recursive: true, force: true });
    }
  }
  for (const entry of await readdir(bundleDirectory)) {
    const path = join(bundleDirectory, entry);
    if (path !== bundlePath && entry.endsWith(".mjs")) {
      await rm(path, { force: true });
    }
  }
  return bundlePath;
}

async function preparePlugin(
  source: PluginSource,
  bundleCacheRoot: string,
): Promise<PreparedPluginSource> {
  assertId(source.id, "Plugin path ID");
  assertText(source.cacheKey, "Plugin cache key", 128);
  const files = await capturePlugin(source);
  const { manifest, entrypoint, capabilities } = parseManifest(source, files);
  const path = await bundlePlugin(source, files, entrypoint, bundleCacheRoot);
  return {
    ...source,
    name: manifest.name,
    version: manifest.version,
    path,
    capabilities,
  };
}

function normalizeInput(input: unknown, commandId: string, seen: Set<string>): unknown {
  if (!input || typeof input !== "object") {
    throw new Error(`Command ${commandId} has an invalid input`);
  }
  const value = input as Record<string, unknown>;
  assertId(value.id, `Input ID for ${commandId}`);
  if (seen.has(value.id)) throw new Error(`Duplicate input ID ${value.id}`);
  seen.add(value.id);
  assertText(value.label, `Input label for ${commandId}`, 200);
  if (value.type === "text") {
    if (
      value.maxLength !== undefined &&
      (!Number.isInteger(value.maxLength) || Number(value.maxLength) < 1 || Number(value.maxLength) > 16_384)
    ) {
      throw new Error(`Text input ${value.id} has an invalid maxLength`);
    }
    const maxLength = value.maxLength === undefined ? 1_024 : Number(value.maxLength);
    const defaultValue = optionalString(
      value.defaultValue,
      `Default value for ${value.id}`,
      maxLength,
    );
    return {
      type: "text",
      id: value.id,
      label: value.label,
      placeholder: optionalText(value.placeholder, `Placeholder for ${value.id}`, 300),
      defaultValue,
      required: value.required === true,
      maxLength,
    };
  }
  if (value.type === "select") {
    if (!Array.isArray(value.options) || value.options.length === 0 || value.options.length > 128) {
      throw new Error(`Select input ${value.id} must have 1 to 128 options`);
    }
    const optionValues = new Set<string>();
    const options = value.options.map((raw) => {
      if (!raw || typeof raw !== "object") throw new Error(`Invalid option in ${value.id}`);
      const option = raw as Record<string, unknown>;
      assertText(option.value, `Option value for ${value.id}`, 1_024);
      assertText(option.label, `Option label for ${value.id}`, 200);
      if (optionValues.has(option.value)) throw new Error(`Duplicate option ${option.value}`);
      optionValues.add(option.value);
      return {
        value: option.value,
        label: option.label,
        keywords: normalizeKeywords(option.keywords, `Option keywords for ${value.id}`),
        status: optionalText(option.status, `Option status for ${value.id}`, 200),
      };
    });
    const defaultValue = optionalText(value.defaultValue, `Default value for ${value.id}`, 1_024);
    if (defaultValue !== undefined && !optionValues.has(defaultValue)) {
      throw new Error(`Select input ${value.id} defaultValue must match an option`);
    }
    return {
      type: "select",
      id: value.id,
      label: value.label,
      placeholder: optionalText(value.placeholder, `Placeholder for ${value.id}`, 300),
      defaultValue,
      required: value.required !== false,
      options,
    };
  }
  if (value.type === "confirm") {
    if (value.defaultValue !== undefined && typeof value.defaultValue !== "boolean") {
      throw new Error(`Confirm input ${value.id} defaultValue must be a boolean`);
    }
    return {
      type: "confirm",
      id: value.id,
      label: value.label,
      defaultValue: value.defaultValue !== false,
    };
  }
  throw new Error(`Input ${value.id} has unsupported type ${String(value.type)}`);
}

function normalizePlugin(
  candidate: unknown,
  source: PreparedPluginSource,
): {
  commands: Record<string, unknown>[];
  events: Record<string, unknown>[];
  views: Record<string, unknown>[];
  settings: Record<string, unknown>[];
} {
  if (!candidate || typeof candidate !== "object") {
    throw new Error("Default export must be definePlugin({...})");
  }
  const definition = candidate as PluginDefinition;
  if (!Array.isArray(definition.commands) || definition.commands.length > 256) {
    throw new Error("Plugin commands must be an array with at most 256 entries");
  }
  const ids = new Set<string>();
  commandHandlers = new Map();
  const commands = definition.commands.map((command) => {
    if (!command || typeof command !== "object") throw new Error("Invalid plugin command");
    assertId(command.id, "Command ID");
    if (ids.has(command.id)) throw new Error(`Duplicate command ID ${command.id}`);
    ids.add(command.id);
    assertText(command.title, `Command title for ${command.id}`);
    if (typeof command.run !== "function") {
      throw new Error(`Command ${command.id} must define run()`);
    }
    if (command.enabled !== undefined && typeof command.enabled !== "boolean") {
      throw new Error(`Command ${command.id} enabled must be a boolean`);
    }
    const inputIds = new Set<string>();
    const inputs = Array.isArray(command.inputs)
      ? command.inputs.map((input) => normalizeInput(input, command.id, inputIds))
      : [];
    if (inputs.length > 16) throw new Error(`Command ${command.id} has too many inputs`);
    commandHandlers.set(command.id, command.run);
    if (
      command.timeoutMs !== undefined &&
      (!Number.isInteger(command.timeoutMs) || command.timeoutMs < 100 || command.timeoutMs > 30_000)
    ) {
      throw new Error(`Command ${command.id} timeoutMs must be between 100 and 30000`);
    }
    const timeoutMs = command.timeoutMs ?? 10_000;
    const icons = [
      "command",
      "play",
      "terminal",
      "folder",
      "link",
      "clipboard",
      "settings",
      "info",
    ];
    if (command.icon !== undefined && !icons.includes(command.icon)) {
      throw new Error(`Command ${command.id} has an unsupported icon`);
    }
    const supportedPlacements = [
      "commandPalette",
      "terminalContextMenu",
      "tabContextMenu",
    ];
    const placements = command.placements ?? ["commandPalette"];
    if (
      !Array.isArray(placements) ||
      placements.length > supportedPlacements.length ||
      placements.some(
        (placement) =>
          typeof placement !== "string" || !supportedPlacements.includes(placement),
      ) ||
      new Set(placements).size !== placements.length
    ) {
      throw new Error(`Command ${command.id} has invalid or duplicate placements`);
    }
    const disabledReason =
      command.enabled === false
        ? optionalText(
            command.disabledReason,
            `Disabled reason for ${command.id}`,
            300,
          ) || "Disabled by plugin"
        : undefined;
    return {
      pluginId: source.id,
      pluginName: source.name,
      id: command.id,
      title: command.title,
      placements,
      keywords: normalizeKeywords(command.keywords, `Keywords for ${command.id}`),
      status: optionalText(command.status, `Status for ${command.id}`, 200),
      disabledReason,
      icon: command.icon || "command",
      inputs,
      timeoutMs,
    };
  });
  const events = normalizeEvents(definition.events, source);
  const views = normalizeViews(definition.views, source);
  const settings = normalizeSettings(definition.settings);
  plugin = definition;
  return { commands, events, views, settings };
}

const PLUGIN_EVENT_NAMES = [
  "terminal.ready",
  "tab.activated",
  "workingDirectory.changed",
  "command.finished",
] as const satisfies readonly PluginEventName[];

function normalizeEvents(
  value: unknown,
  source: PreparedPluginSource,
): Record<string, unknown>[] {
  eventHandlers = new Map();
  if (value === undefined) return [];
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Plugin events must be an object");
  }
  return Object.entries(value as Record<string, unknown>).map(([event, handler]) => {
    if (!PLUGIN_EVENT_NAMES.includes(event as PluginEventName)) {
      throw new Error(`Plugin event ${event} is not supported`);
    }
    if (typeof handler !== "function") {
      throw new Error(`Plugin event ${event} must be a function`);
    }
    const eventName = event as PluginEventName;
    eventHandlers.set(eventName, handler as PluginEventHandler);
    return {
      pluginId: source.id,
      event: eventName,
      timeoutMs: 10_000,
    };
  });
}

function normalizeViews(
  value: unknown,
  source: PreparedPluginSource,
): Record<string, unknown>[] {
  viewHandlers = new Map();
  if (value === undefined) return [];
  requireCapability("native-ui", "Declaring plugin views");
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Plugin views must be an object");
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length > MAX_PLUGIN_VIEWS) {
    throw new Error(`Plugin views must have at most ${MAX_PLUGIN_VIEWS} entries`);
  }
  return entries.map(([id, candidate]) => {
    assertId(id, "View ID");
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
      throw new Error(`View ${id} must be an object`);
    }
    const view = candidate as PluginViewDefinition;
    assertText(view.title, `View ${id} title`, 300);
    if (typeof view.render !== "function") {
      throw new Error(`View ${id} must define render()`);
    }
    if (view.onAction !== undefined && typeof view.onAction !== "function") {
      throw new Error(`View ${id} onAction must be a function`);
    }
    if (
      view.timeoutMs !== undefined &&
      (!Number.isInteger(view.timeoutMs) || view.timeoutMs < 100 || view.timeoutMs > 30_000)
    ) {
      throw new Error(`View ${id} timeoutMs must be between 100 and 30000`);
    }
    viewHandlers.set(id, view);
    return {
      pluginId: source.id,
      pluginName: source.name,
      id,
      title: view.title,
      timeoutMs: view.timeoutMs ?? 10_000,
    };
  });
}

function normalizeSettings(value: unknown): Record<string, unknown>[] {
  if (value === undefined) return [];
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Plugin settings must be an object");
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length > 64) throw new Error("Plugin settings must have at most 64 entries");
  return entries.map(([id, candidate]) => {
    assertId(id, "Setting ID");
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
      throw new Error(`Setting ${id} must be an object`);
    }
    const setting = candidate as PluginSettingDefinition;
    assertText(setting.title, `Setting title for ${id}`, 200);
    const description = optionalText(
      setting.description,
      `Setting description for ${id}`,
      500,
    );
    if (setting.type === "toggle") {
      if (setting.defaultValue !== undefined && typeof setting.defaultValue !== "boolean") {
        throw new Error(`Setting ${id} defaultValue must be a boolean`);
      }
      return {
        id,
        type: "toggle",
        title: setting.title,
        description,
        defaultValue: setting.defaultValue === true,
      };
    }
    if (setting.type === "text" || setting.type === "secret") {
      const maxLength = setting.maxLength ?? 4_096;
      if (!Number.isInteger(maxLength) || maxLength < 1 || maxLength > 4_096) {
        throw new Error(`Setting ${id} maxLength must be between 1 and 4096`);
      }
      const placeholder = optionalText(
        setting.placeholder,
        `Setting placeholder for ${id}`,
        300,
      );
      if (setting.type === "secret") {
        if (setting.defaultValue !== undefined) {
          throw new Error(`Secret setting ${id} cannot define defaultValue`);
        }
        return {
          id,
          type: "secret",
          title: setting.title,
          description,
          placeholder,
          maxLength,
        };
      }
      if (setting.defaultValue !== undefined && typeof setting.defaultValue !== "string") {
        throw new Error(`Setting ${id} defaultValue must be text`);
      }
      const defaultValue = String(setting.defaultValue ?? "");
      if (textLength(defaultValue) > maxLength) {
        throw new Error(`Setting ${id} defaultValue exceeds maxLength`);
      }
      return {
        id,
        type: "text",
        title: setting.title,
        description,
        placeholder,
        defaultValue,
        maxLength,
      };
    }
    if (setting.type === "select") {
      if (!Array.isArray(setting.options) || setting.options.length < 1 || setting.options.length > 128) {
        throw new Error(`Setting ${id} must have between 1 and 128 options`);
      }
      const seen = new Set<string>();
      const options = setting.options.map((option) => {
        if (!option || typeof option !== "object") {
          throw new Error(`Setting ${id} has an invalid option`);
        }
        assertText(option.value, `Setting option value for ${id}`, 1_024);
        assertText(option.label, `Setting option label for ${id}`, 200);
        if (seen.has(option.value)) {
          throw new Error(`Setting ${id} has duplicate option ${option.value}`);
        }
        seen.add(option.value);
        return { value: option.value, label: option.label };
      });
      if (setting.defaultValue !== undefined && typeof setting.defaultValue !== "string") {
        throw new Error(`Setting ${id} defaultValue must be text`);
      }
      const defaultValue = String(setting.defaultValue ?? options[0].value);
      if (!seen.has(defaultValue)) {
        throw new Error(`Setting ${id} defaultValue must match an option`);
      }
      return {
        id,
        type: "select",
        title: setting.title,
        description,
        defaultValue,
        options,
      };
    }
    throw new Error(`Setting ${id} has unsupported type ${String(setting.type)}`);
  });
}

function normalizeActions(value: unknown): unknown[] {
  let actions: unknown[];
  if (value === undefined || value === null) actions = [];
  else if (Array.isArray(value)) actions = value;
  else if (
    typeof value === "object" &&
    Array.isArray((value as { actions?: unknown }).actions)
  ) {
    actions = (value as { actions: unknown[] }).actions;
  } else actions = [value];

  if (
    actions.some((action) =>
      action !== null &&
      typeof action === "object" &&
      (action as { type?: unknown }).type === "view.open"
    )
  ) {
    requireCapability("native-ui", "Opening plugin views");
  }
  return actions;
}

function assertStorageKey(value: unknown): asserts value is string {
  if (
    typeof value !== "string" ||
    value.trim() === "" ||
    textLength(value) > 200 ||
    value.includes("\0")
  ) {
    throw new Error("Plugin storage key must be a non-empty string up to 200 characters");
  }
}

function emptyStorage(): Record<string, PluginJsonValue> {
  return Object.create(null) as Record<string, PluginJsonValue>;
}

function createPluginStorage(storageDirectory: string): PluginStorage {
  const storagePath = join(storageDirectory, "storage.json");
  let values: Record<string, PluginJsonValue> | undefined;
  let operations = Promise.resolve();

  const enqueue = <T>(operation: () => Promise<T>): Promise<T> => {
    const result = operations.then(operation, operation);
    operations = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  };
  const load = async (): Promise<Record<string, PluginJsonValue>> => {
    if (values) return values;
    let contents: Uint8Array;
    try {
      contents = new Uint8Array(await readFile(storagePath));
    } catch (error) {
      if ((error as { code?: string }).code === "ENOENT") {
        values = emptyStorage();
        return values;
      }
      throw error;
    }
    if (contents.byteLength > MAX_STORAGE_BYTES) {
      throw new Error("Plugin storage exceeds the 1 MiB limit");
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(Buffer.from(contents).toString("utf8"));
    } catch {
      throw new Error("Plugin storage contains invalid JSON");
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("Plugin storage must contain a JSON object");
    }
    const entries = Object.entries(parsed as Record<string, PluginJsonValue>);
    if (entries.length > MAX_STORAGE_ENTRIES) {
      throw new Error(`Plugin storage exceeds ${MAX_STORAGE_ENTRIES} entries`);
    }
    values = emptyStorage();
    for (const [key, value] of entries) {
      assertStorageKey(key);
      values[key] = value;
    }
    return values;
  };
  const persist = async (next: Record<string, PluginJsonValue>): Promise<void> => {
    const contents = JSON.stringify(next);
    if (Buffer.byteLength(contents) > MAX_STORAGE_BYTES) {
      throw new Error("Plugin storage exceeds the 1 MiB limit");
    }
    await mkdir(storageDirectory, { recursive: true });
    const temporaryPath = join(
      storageDirectory,
      `.storage-${process.pid}-${Date.now()}-${Math.random().toString(16).slice(2)}.tmp`,
    );
    try {
      await writeFile(temporaryPath, contents, { flag: "wx" });
      await rename(temporaryPath, storagePath);
    } finally {
      await rm(temporaryPath, { force: true });
    }
  };
  const cloneValue = (value: PluginJsonValue): PluginJsonValue =>
    structuredClone(value);

  return Object.freeze({
    get: <T = PluginJsonValue>(key: string): Promise<T | undefined> =>
      enqueue(async () => {
        assertStorageKey(key);
        const current = await load();
        if (!Object.hasOwn(current, key)) return undefined;
        return cloneValue(current[key]) as unknown as T;
      }),
    set: (key: string, value: PluginJsonValue): Promise<void> =>
      enqueue(async () => {
        assertStorageKey(key);
        let normalized: PluginJsonValue;
        try {
          const serialized = JSON.stringify(value);
          if (serialized === undefined) throw new Error();
          normalized = JSON.parse(serialized) as PluginJsonValue;
        } catch {
          throw new Error("Plugin storage values must be JSON-serializable");
        }
        const current = await load();
        const next = Object.assign(emptyStorage(), current);
        next[key] = normalized;
        if (Object.keys(next).length > MAX_STORAGE_ENTRIES) {
          throw new Error(`Plugin storage exceeds ${MAX_STORAGE_ENTRIES} entries`);
        }
        await persist(next);
        values = next;
      }),
    delete: (key: string): Promise<boolean> =>
      enqueue(async () => {
        assertStorageKey(key);
        const current = await load();
        if (!Object.hasOwn(current, key)) return false;
        const next = Object.assign(emptyStorage(), current);
        delete next[key];
        await persist(next);
        values = next;
        return true;
      }),
    clear: (): Promise<void> =>
      enqueue(async () => {
        await rm(storagePath, { force: true });
        values = emptyStorage();
      }),
  });
}

async function createPluginServices(
  source: PreparedPluginSource,
  dataRoot: string,
  cacheRoot: string,
): Promise<PluginServices> {
  if (!isAbsolute(dataRoot) || !isAbsolute(cacheRoot)) {
    throw new Error("Plugin storage paths must be absolute");
  }
  if (!source.capabilities.includes("storage")) {
    const unavailable = (): Error =>
      new Error("Plugin storage requires capability `storage` in plugin.json");
    const paths = {} as PluginPaths;
    Object.defineProperties(paths, {
      dataDirectory: {
        enumerable: true,
        get: () => {
          throw unavailable();
        },
      },
      cacheDirectory: {
        enumerable: true,
        get: () => {
          throw unavailable();
        },
      },
    });
    return Object.freeze({
      storage: Object.freeze({
        get: async () => {
          throw unavailable();
        },
        set: async () => {
          throw unavailable();
        },
        delete: async () => {
          throw unavailable();
        },
        clear: async () => {
          throw unavailable();
        },
      }),
      paths: Object.freeze(paths),
    });
  }
  const storageDirectory = join(dataRoot, source.id);
  const dataDirectory = join(storageDirectory, "files");
  const cacheDirectory = join(cacheRoot, source.id);
  await Promise.all([
    mkdir(dataDirectory, { recursive: true }),
    mkdir(cacheDirectory, { recursive: true }),
  ]);
  return Object.freeze({
    storage: createPluginStorage(storageDirectory),
    paths: Object.freeze({ dataDirectory, cacheDirectory }),
  });
}

function createPluginContext(
  value: unknown,
  emittedActions: unknown[],
  services: PluginServices,
): PluginContext {
  const context =
    typeof value === "object" && value !== null && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {};
  const settingValues =
    context.settings &&
    typeof context.settings === "object" &&
    !Array.isArray(context.settings)
      ? (context.settings as Record<string, string | boolean>)
      : {};
  const activeTab =
    context.activeTab &&
    typeof context.activeTab === "object" &&
    !Array.isArray(context.activeTab)
      ? Object.freeze({ ...(context.activeTab as Record<string, unknown>) })
      : undefined;
  const activePane =
    context.activePane &&
    typeof context.activePane === "object" &&
    !Array.isArray(context.activePane)
      ? Object.freeze({ ...(context.activePane as Record<string, unknown>) })
      : undefined;
  const toast = (level: PluginToastLevel, message: unknown) => {
    assertText(message, "Toast message", 4_096);
    emittedActions.push({ type: "toast", level, message });
  };

  return Object.freeze({
    ...context,
    ...(activeTab ? { activeTab } : {}),
    ...(activePane ? { activePane } : {}),
    ...services,
    settings: Object.freeze({
      get: <T = string | boolean>(key: string): T | undefined => {
        assertId(key, "Setting ID");
        if (!Object.hasOwn(settingValues, key)) return undefined;
        return structuredClone(settingValues[key]) as T;
      },
    }),
    toasts: Object.freeze({
      info: (message: string) => toast("info", message),
      success: (message: string) => toast("success", message),
      warning: (message: string) => toast("warning", message),
      error: (message: string) => toast("error", message),
    }),
  });
}

function hasInteractiveViewNode(nodes: Record<string, unknown>[]): boolean {
  return nodes.some((node) => {
    if (node.type === "button" || node.type === "checkbox") return true;
    if (node.type === "textInput" && node.submit !== undefined) return true;
    return Array.isArray(node.children)
      && hasInteractiveViewNode(node.children as Record<string, unknown>[]);
  });
}

async function renderPluginView(
  viewId: string,
  context: PluginContext,
  emittedActions: unknown[],
): Promise<Record<string, unknown>[]> {
  const view = viewHandlers.get(viewId);
  if (!view) throw new Error(`View ${viewId} is not registered`);
  const nodes = normalizeViewDocument(await view.render({ context }));
  if (!view.onAction && hasInteractiveViewNode(nodes)) {
    throw new Error(`View ${viewId} renders actions but does not define onAction()`);
  }
  return nodes;
}

function normalizeViewAction(value: unknown): PluginViewAction {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Plugin view action is invalid");
  }
  const action = value as Record<string, unknown>;
  assertId(action.id, "Plugin view action ID");
  assertId(action.controlId, "Plugin view control ID");
  const payload = optionalText(action.payload, "Plugin view action payload", 1_024);
  let normalizedValue: PluginViewValue | undefined;
  if (action.value !== undefined) {
    if (typeof action.value !== "string" && typeof action.value !== "boolean") {
      throw new Error("Plugin view action value must be text or a boolean");
    }
    if (typeof action.value === "string" && textLength(action.value) > 4_096) {
      throw new Error("Plugin view action value exceeds 4096 characters");
    }
    normalizedValue = action.value;
  }
  return {
    id: action.id,
    controlId: action.controlId,
    ...(payload === undefined ? {} : { payload }),
    ...(normalizedValue === undefined ? {} : { value: normalizedValue }),
  };
}

function normalizeViewValues(value: unknown): Record<string, PluginViewValue> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Plugin view values are invalid");
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length > MAX_VIEW_VALUES) {
    throw new Error("Plugin view submitted too many values");
  }
  const normalized: Record<string, PluginViewValue> = Object.create(null);
  for (const [id, candidate] of entries) {
    assertId(id, "Plugin view value ID");
    if (typeof candidate !== "string" && typeof candidate !== "boolean") {
      throw new Error(`Plugin view value ${id} must be text or a boolean`);
    }
    if (typeof candidate === "string" && textLength(candidate) > 4_096) {
      throw new Error(`Plugin view value ${id} exceeds 4096 characters`);
    }
    normalized[id] = candidate;
  }
  return normalized;
}

async function handle(message: Record<string, unknown>): Promise<unknown> {
  if (message.type === "load") {
    const pluginSource = message.source as PluginSource;
    const source = await preparePlugin(
      pluginSource,
      String(message.bundleCacheRoot || ""),
    );
    pluginCapabilities = new Set(source.capabilities);
    pluginServices = await createPluginServices(
      source,
      String(message.pluginDataRoot || ""),
      String(message.pluginCacheRoot || ""),
    );
    const moduleUrl = `${pathToFileURL(source.path).href}?termy=${source.cacheKey}`;
    let loaded: Record<string, unknown>;
    try {
      loaded = await import(moduleUrl);
    } catch (error) {
      await rm(source.path, { force: true });
      throw error;
    }
    return normalizePlugin(loaded.default, source);
  }
  if (message.type === "invoke") {
    if (!plugin) throw new Error("Plugin is not loaded");
    if (!pluginServices) throw new Error("Plugin services are unavailable");
    const commandId = String(message.commandId || "");
    const run = commandHandlers.get(commandId);
    if (!run) throw new Error(`Command ${commandId} is not registered`);
    const emittedActions: unknown[] = [];
    const value = await run({
      inputs: (message.inputs || {}) as Record<string, unknown>,
      context: createPluginContext(message.context, emittedActions, pluginServices),
    });
    return { actions: [...emittedActions, ...normalizeActions(value)] };
  }
  if (message.type === "event") {
    if (!plugin) throw new Error("Plugin is not loaded");
    if (!pluginServices) throw new Error("Plugin services are unavailable");
    const event = Object.freeze({ ...(message.event as Record<string, unknown>) });
    const eventName = String(event?.type || "") as PluginEventName;
    const run = eventHandlers.get(eventName);
    if (!run) throw new Error(`Plugin is not registered for ${eventName}`);
    const emittedActions: unknown[] = [];
    const value = await run({
      event,
      context: createPluginContext(message.context, emittedActions, pluginServices),
    });
    return { actions: [...emittedActions, ...normalizeActions(value)] };
  }
  if (message.type === "view.render") {
    if (!plugin) throw new Error("Plugin is not loaded");
    if (!pluginServices) throw new Error("Plugin services are unavailable");
    const viewId = String(message.viewId || "");
    const emittedActions: unknown[] = [];
    const context = createPluginContext(message.context, emittedActions, pluginServices);
    const nodes = await renderPluginView(viewId, context, emittedActions);
    return { nodes, actions: emittedActions };
  }
  if (message.type === "view.action") {
    if (!plugin) throw new Error("Plugin is not loaded");
    if (!pluginServices) throw new Error("Plugin services are unavailable");
    const viewId = String(message.viewId || "");
    const view = viewHandlers.get(viewId);
    if (!view) throw new Error(`View ${viewId} is not registered`);
    if (!view.onAction) throw new Error(`View ${viewId} does not handle actions`);
    const emittedActions: unknown[] = [];
    const context = createPluginContext(message.context, emittedActions, pluginServices);
    const value = await view.onAction({
      action: normalizeViewAction(message.action),
      values: normalizeViewValues(message.values),
      context,
    });
    const nodes = await renderPluginView(viewId, context, emittedActions);
    return {
      nodes,
      actions: [...emittedActions, ...normalizeActions(value)],
    };
  }
  throw new Error(`Unknown Worker request ${String(message.type)}`);
}

self.onmessage = (event: MessageEvent<Record<string, unknown>>) => {
  const message = event.data;
  queue = queue.then(async () => {
    try {
      const result = await handle(message);
      postMessage({ id: message.id, ok: true, result });
    } catch (error) {
      process.stderr.write(
        `[termy plugin ${process.env.TERMY_PLUGIN_ID || "unknown"}] ${errorMessage(error)}\n`,
      );
      postMessage({ id: message.id, ok: false, error: errorMessage(error) });
    }
  });
};

export {};
