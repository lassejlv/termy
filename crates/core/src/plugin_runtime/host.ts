// Bun-side coordinator for isolated Termy plugin Workers.
import { mkdir, readdir, rm } from "node:fs/promises";
import { createConnection } from "node:net";
import { join } from "node:path";
import { createInterface } from "node:readline";

type PluginSource = { id: string; root: string; cacheKey: string };
type HostRequest =
  | { id: number; type: "load"; plugins: PluginSource[] }
  | {
      id: number;
      type: "invoke";
      pluginId: string;
      commandId: string;
      revision: string;
      inputs: Record<string, unknown>;
      context: Record<string, unknown>;
    }
  | {
      id: number;
      type: "input.options";
      pluginId: string;
      commandId: string;
      inputId: string;
      revision: string;
      query: string;
      context: Record<string, unknown>;
    }
  | {
      id: number;
      type: "event";
      pluginId: string;
      revision: string;
      event: Record<string, unknown>;
      context: Record<string, unknown>;
    }
  | {
      id: number;
      type: "view.render";
      pluginId: string;
      viewId: string;
      revision: string;
      params: Record<string, unknown>;
      context: Record<string, unknown>;
    }
  | {
      id: number;
      type: "view.action";
      pluginId: string;
      viewId: string;
      revision: string;
      params: Record<string, unknown>;
      action: Record<string, unknown>;
      values: Record<string, unknown>;
      context: Record<string, unknown>;
    }
  | { id: number; type: "cancel"; requestId: number };

type InvocationCommand = { id: string; timeoutMs: number };
type InvocationEvent = { event: string; timeoutMs: number };
type InvocationView = { id: string; timeoutMs: number };
type WorkerRecord = {
  worker: Worker;
  source: PluginSource;
  healthy: boolean;
  invokeQueue: Promise<void>;
  invocationCount: number;
  pending: Map<
    number,
    {
      resolve: (value: unknown) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
      hostRequestId?: number;
      onProgress?: (progress: Record<string, unknown>) => void;
    }
  >;
  commands: unknown[];
  events: unknown[];
  views: unknown[];
  settings: unknown[];
  invocationCommands: InvocationCommand[];
  invocationEvents: InvocationEvent[];
  invocationViews: InvocationView[];
};
type PluginLoadResult = {
  source: PluginSource;
  reused: boolean;
  record?: WorkerRecord;
  commands?: unknown[];
  events?: unknown[];
  views?: unknown[];
  settings?: unknown[];
  error?: string;
};

const workerPath = process.env.TERMY_PLUGIN_WORKER_PATH;
const protocolPort = Number(process.env.TERMY_PLUGIN_PROTOCOL_PORT);
const protocolSecret = process.env.TERMY_PLUGIN_PROTOCOL_SECRET;
if (
  !workerPath ||
  !Number.isSafeInteger(protocolPort) ||
  protocolPort <= 0 ||
  protocolPort > 65_535 ||
  !protocolSecret
) {
  process.stderr.write("Termy plugin runtime configuration is missing\n");
  process.exit(1);
}
delete process.env.TERMY_PLUGIN_PROTOCOL_PORT;
delete process.env.TERMY_PLUGIN_PROTOCOL_SECRET;

const records = new Map<string, WorkerRecord>();
const activeWorkerRequests = new Map<
  number,
  { record: WorkerRecord; workerRequestId: number }
>();
const cancelledHostRequests = new Set<number>();
let nextWorkerRequestId = 1;
let loadQueue = Promise.resolve<unknown>(undefined);
const MAX_PROTOCOL_BYTES = 1024 * 1024;
const LOAD_TIMEOUT_MS = 10_000;
const DEFAULT_TIMEOUT_MS = 10_000;
const MAX_INVOKE_QUEUE_WAIT_MS = 30_000;
const MAX_PLUGIN_INVOCATIONS = 8;
const MAX_PLUGINS = 32;
const MAX_LOAD_CONCURRENCY = 4;
const MAX_PLUGIN_COMMANDS = 512;
const bundleCacheRoot = join(process.cwd(), ".termy-cache", "bundles");
const pluginDataRoot = join(process.cwd(), ".termy-data");
const pluginCacheRoot = join(process.cwd(), ".termy-cache", "data");

class WorkerExecutionTimeoutError extends Error {}

class WorkerQueueTimeoutError extends Error {}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function terminateRecord(record: WorkerRecord): void {
  record.healthy = false;
  record.worker.terminate();
  for (const pending of record.pending.values()) {
    clearTimeout(pending.timer);
    if (pending.hostRequestId !== undefined) {
      activeWorkerRequests.delete(pending.hostRequestId);
    }
    pending.reject(new Error("Plugin Worker stopped"));
  }
  record.pending.clear();
}

function terminateAll(): void {
  for (const record of records.values()) terminateRecord(record);
  records.clear();
}

function workerEnvironment(pluginId: string): Record<string, string> {
  const allowed = [
    "HOME",
    "USERPROFILE",
    "PATH",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "LANG",
    "LC_ALL",
    "TERM",
  ];
  const env: Record<string, string> = {
    TERMY_PLUGIN_ID: pluginId,
    DO_NOT_TRACK: "1",
  };
  for (const key of allowed) {
    const value = process.env[key];
    if (value !== undefined) env[key] = value;
  }
  return env;
}

function createRecord(source: PluginSource): WorkerRecord {
  const worker = new Worker(workerPath, {
    smol: true,
    env: workerEnvironment(source.id),
  });
  const record: WorkerRecord = {
    worker,
    source,
    healthy: true,
    invokeQueue: Promise.resolve(),
    invocationCount: 0,
    pending: new Map(),
    commands: [],
    events: [],
    views: [],
    settings: [],
    invocationCommands: [],
    invocationEvents: [],
    invocationViews: [],
  };
  worker.onmessage = (event) => {
    const message = event.data as {
      id?: number;
      ok?: boolean;
      result?: unknown;
      error?: string;
      progress?: Record<string, unknown>;
    };
    if (typeof message.id !== "number") return;
    const pending = record.pending.get(message.id);
    if (!pending) return;
    if (message.progress) {
      pending.onProgress?.(message.progress);
      return;
    }
    record.pending.delete(message.id);
    if (pending.hostRequestId !== undefined) {
      activeWorkerRequests.delete(pending.hostRequestId);
    }
    clearTimeout(pending.timer);
    if (message.ok) pending.resolve(message.result);
    else pending.reject(new Error(message.error || "Plugin Worker failed"));
  };
  worker.onerror = (event) => {
    record.healthy = false;
    const detail = event.error ? errorMessage(event.error) : event.message;
    const error = new Error(detail || `Plugin ${source.id} crashed`);
    process.stderr.write(`[termy plugin ${source.id}] Worker crashed: ${error.message}\n`);
    for (const pending of record.pending.values()) {
      clearTimeout(pending.timer);
      if (pending.hostRequestId !== undefined) {
        activeWorkerRequests.delete(pending.hostRequestId);
      }
      pending.reject(error);
    }
    record.pending.clear();
  };
  return record;
}

function requestWorker(
  record: WorkerRecord,
  message: Record<string, unknown>,
  timeoutMs: number,
  hostRequestId?: number,
  onProgress?: (progress: Record<string, unknown>) => void,
): Promise<unknown> {
  const id = nextWorkerRequestId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      record.pending.delete(id);
      if (hostRequestId !== undefined) activeWorkerRequests.delete(hostRequestId);
      record.healthy = false;
      record.worker.terminate();
      reject(new WorkerExecutionTimeoutError(`Plugin timed out after ${timeoutMs} ms`));
    }, timeoutMs);
    record.pending.set(id, { resolve, reject, timer, hostRequestId, onProgress });
    if (hostRequestId !== undefined) {
      activeWorkerRequests.set(hostRequestId, { record, workerRequestId: id });
    }
    record.worker.postMessage({ ...message, id });
  });
}

function enqueueWorkerInvocation(
  record: WorkerRecord,
  message: Record<string, unknown>,
  timeoutMs: number,
  hostRequestId: number,
  onProgress: (progress: Record<string, unknown>) => void,
): Promise<unknown> {
  if (!record.healthy) {
    return Promise.reject(new Error("Plugin Worker is unavailable"));
  }
  if (record.invocationCount >= MAX_PLUGIN_INVOCATIONS) {
    return Promise.reject(
      new Error(`Plugin has too many pending invocations; maximum is ${MAX_PLUGIN_INVOCATIONS}`),
    );
  }
  record.invocationCount += 1;
  return new Promise((resolve, reject) => {
    let expired = false;
    let started = false;
    const queueTimer = setTimeout(() => {
      if (started) return;
      expired = true;
      reject(
        new WorkerQueueTimeoutError(
          `Plugin invocation queue timed out after ${MAX_INVOKE_QUEUE_WAIT_MS} ms`,
        ),
      );
    }, MAX_INVOKE_QUEUE_WAIT_MS);

    const run = async () => {
      if (expired || cancelledHostRequests.has(hostRequestId)) {
        record.invocationCount -= 1;
        if (!expired) reject(new Error("Plugin invocation cancelled"));
        return;
      }
      started = true;
      clearTimeout(queueTimer);
      if (!record.healthy) {
        record.invocationCount -= 1;
        reject(new Error("Plugin Worker is unavailable"));
        return;
      }
      try {
        resolve(await requestWorker(record, message, timeoutMs, hostRequestId, onProgress));
      } catch (error) {
        reject(error);
      } finally {
        record.invocationCount -= 1;
      }
    };
    const queued = record.invokeQueue.then(run, run);
    record.invokeQueue = queued.then(
      () => undefined,
      () => undefined,
    );
  });
}

async function cleanupBuildArtifacts(source: PluginSource): Promise<void> {
  const bundleDirectory = join(bundleCacheRoot, source.id);
  let entries;
  try {
    entries = await readdir(bundleDirectory, { withFileTypes: true });
  } catch (error) {
    if ((error as { code?: string }).code === "ENOENT") return;
    throw error;
  }
  await Promise.all(
    entries.map(async (entry) => {
      const staleCapture = entry.name.startsWith(".capture-");
      const staleTemporaryBundle = entry.name.endsWith(".tmp");
      if (staleCapture || staleTemporaryBundle) {
        await rm(join(bundleDirectory, entry.name), {
          recursive: staleCapture,
          force: true,
        });
      }
    }),
  );
}

function invocationCommands(value: unknown): InvocationCommand[] {
  if (!value || typeof value !== "object") {
    throw new Error("Plugin Worker returned an invalid load result");
  }
  const commands = (value as { commands?: unknown }).commands;
  if (!Array.isArray(commands)) {
    throw new Error("Plugin Worker returned an invalid command list");
  }
  return commands.map((entry) => {
    if (!entry || typeof entry !== "object") {
      throw new Error("Plugin Worker returned an invalid command");
    }
    const command = entry as Record<string, unknown>;
    if (typeof command.id !== "string" || typeof command.timeoutMs !== "number") {
      throw new Error("Plugin Worker returned an invalid command descriptor");
    }
    return { id: command.id, timeoutMs: command.timeoutMs };
  });
}

function invocationEvents(value: unknown): InvocationEvent[] {
  if (!value || typeof value !== "object") {
    throw new Error("Plugin Worker returned an invalid load result");
  }
  const events = (value as { events?: unknown }).events;
  if (!Array.isArray(events)) {
    throw new Error("Plugin Worker returned an invalid event subscription list");
  }
  return events.map((entry) => {
    if (!entry || typeof entry !== "object") {
      throw new Error("Plugin Worker returned an invalid event subscription");
    }
    const event = entry as Record<string, unknown>;
    if (typeof event.event !== "string" || typeof event.timeoutMs !== "number") {
      throw new Error("Plugin Worker returned an invalid event subscription descriptor");
    }
    return { event: event.event, timeoutMs: event.timeoutMs };
  });
}

function invocationViews(value: unknown): InvocationView[] {
  if (!value || typeof value !== "object") {
    throw new Error("Plugin Worker returned an invalid load result");
  }
  const views = (value as { views?: unknown }).views;
  if (!Array.isArray(views)) {
    throw new Error("Plugin Worker returned an invalid view list");
  }
  return views.map((entry) => {
    if (!entry || typeof entry !== "object") {
      throw new Error("Plugin Worker returned an invalid view");
    }
    const view = entry as Record<string, unknown>;
    if (typeof view.id !== "string" || typeof view.timeoutMs !== "number") {
      throw new Error("Plugin Worker returned an invalid view descriptor");
    }
    return { id: view.id, timeoutMs: view.timeoutMs };
  });
}

async function loadPlugin(source: PluginSource): Promise<PluginLoadResult> {
  try {
    await cleanupBuildArtifacts(source);
  } catch (error) {
    return {
      source,
      reused: false,
      error: `Failed to clean plugin build cache: ${errorMessage(error)}`,
    };
  }
  let record: WorkerRecord;
  try {
    record = createRecord(source);
  } catch (error) {
    return { source, reused: false, error: errorMessage(error) };
  }
  try {
    const result = await requestWorker(
      record,
      {
        type: "load",
        source,
        bundleCacheRoot,
        pluginDataRoot,
        pluginCacheRoot,
      },
      LOAD_TIMEOUT_MS,
    );
    record.invocationCommands = invocationCommands(result);
    record.invocationEvents = invocationEvents(result);
    record.invocationViews = invocationViews(result);
    record.commands = (result as { commands: unknown[] }).commands;
    record.events = (result as { events: unknown[] }).events;
    record.views = (result as { views: unknown[] }).views;
    record.settings = (result as { settings: unknown[] }).settings;
    await cleanupBuildArtifacts(source);
    return {
      source,
      reused: false,
      record,
      commands: record.commands,
      events: record.events,
      views: record.views,
      settings: record.settings,
    };
  } catch (error) {
    terminateRecord(record);
    let detail = errorMessage(error);
    try {
      await cleanupBuildArtifacts(source);
    } catch (cleanupError) {
      detail += `; failed to clean plugin build cache: ${errorMessage(cleanupError)}`;
    }
    return { source, reused: false, error: detail };
  }
}

async function loadOrReusePlugin(source: PluginSource): Promise<PluginLoadResult> {
  const existing = records.get(source.id);
  if (
    existing?.healthy &&
    existing.source.root === source.root &&
    existing.source.cacheKey === source.cacheKey
  ) {
    return {
      source,
      reused: true,
      record: existing,
      commands: existing.commands,
      events: existing.events,
      views: existing.views,
      settings: existing.settings,
    };
  }
  return loadPlugin(source);
}

async function mapWithConcurrency<T, R>(
  values: T[],
  limit: number,
  operation: (value: T) => Promise<R>,
): Promise<R[]> {
  const results = new Array<R>(values.length);
  let nextIndex = 0;
  const workers = Array.from(
    { length: Math.min(limit, values.length) },
    async () => {
      while (nextIndex < values.length) {
        const index = nextIndex++;
        results[index] = await operation(values[index]);
      }
    },
  );
  await Promise.all(workers);
  return results;
}

async function handleLoad(plugins: PluginSource[]): Promise<unknown> {
  if (!Array.isArray(plugins) || plugins.length > MAX_PLUGINS) {
    throw new Error(`Plugin catalog may contain at most ${MAX_PLUGINS} plugins`);
  }
  await mkdir(bundleCacheRoot, { recursive: true });
  const errors: string[] = [];
  const desiredIds = new Set(plugins.map((source) => source.id));
  for (const entry of await readdir(bundleCacheRoot, { withFileTypes: true })) {
    if (entry.isDirectory() && !desiredIds.has(entry.name)) {
      try {
        await rm(join(bundleCacheRoot, entry.name), { recursive: true, force: true });
      } catch (error) {
        errors.push(`cache ${entry.name}: failed to remove stale bundle: ${errorMessage(error)}`);
      }
    }
  }
  const loaded = await mapWithConcurrency(
    plugins,
    MAX_LOAD_CONCURRENCY,
    loadOrReusePlugin,
  );
  const pluginsResult: Array<{
    pluginId: string;
    commands: unknown[];
    events: unknown[];
    views: unknown[];
    settings: unknown[];
  }> = [];
  const nextRecords = new Map<string, WorkerRecord>();
  let commandCount = 0;
  for (const result of loaded) {
    if (
      result.record
      && result.commands
      && result.events
      && result.views
      && result.settings
    ) {
      nextRecords.set(result.source.id, result.record);
      commandCount += result.commands.length;
      pluginsResult.push({
        pluginId: result.source.id,
        commands: result.commands,
        events: result.events,
        views: result.views,
        settings: result.settings,
      });
    } else {
      errors.push(`${result.source.id}: ${result.error || "failed to load"}`);
    }
  }
  if (commandCount > MAX_PLUGIN_COMMANDS) {
    for (const result of loaded) {
      if (result.record && !result.reused) terminateRecord(result.record);
    }
    throw new Error(
      `Plugin catalog has ${commandCount} commands; maximum is ${MAX_PLUGIN_COMMANDS}`,
    );
  }
  for (const [pluginId, record] of records) {
    if (nextRecords.get(pluginId) !== record) terminateRecord(record);
  }
  records.clear();
  for (const [pluginId, record] of nextRecords) records.set(pluginId, record);
  return { plugins: pluginsResult, errors };
}

async function handle(
  request: HostRequest,
  reportProgress: (progress: Record<string, unknown>) => void = () => {},
): Promise<unknown> {
  if (request.type === "load") return handleLoad(request.plugins);
  if (request.type === "cancel") throw new Error("Cancel requests are not invocations");

  const record = records.get(request.pluginId);
  if (!record) throw new Error(`Plugin ${request.pluginId} is not loaded`);
  if (record.source.cacheKey !== request.revision) {
    throw new Error("Plugin changed while its input form was open; run the command again");
  }
  const invocation = request.type === "invoke" || request.type === "input.options"
    ? record.invocationCommands.find((entry) => entry.id === request.commandId)
    : request.type === "event"
      ? record.invocationEvents.find((entry) => entry.event === request.event.type)
      : record.invocationViews.find((entry) => entry.id === request.viewId);
  if (!invocation) {
    const target = request.type === "invoke" || request.type === "input.options"
      ? request.commandId
      : request.type === "event"
        ? String(request.event.type || "")
        : request.viewId;
    throw new Error(`Plugin ${request.pluginId} is not registered for ${target}`);
  }
  const timeoutMs = Math.max(
    100,
    Math.min(30_000, invocation.timeoutMs || DEFAULT_TIMEOUT_MS),
  );
  try {
    return await enqueueWorkerInvocation(
      record,
      request.type === "invoke"
        ? {
            type: "invoke",
            commandId: request.commandId,
            inputs: request.inputs,
            context: request.context,
          }
        : request.type === "input.options"
          ? {
              type: "input.options",
              commandId: request.commandId,
              inputId: request.inputId,
              query: request.query,
              context: request.context,
            }
        : request.type === "event"
          ? {
              type: "event",
              event: request.event,
              context: request.context,
            }
          : request.type === "view.render"
            ? {
                type: "view.render",
                viewId: request.viewId,
                params: request.params,
                context: request.context,
              }
            : {
                type: "view.action",
                viewId: request.viewId,
                params: request.params,
                action: request.action,
                values: request.values,
                context: request.context,
              },
      timeoutMs,
      request.id,
      reportProgress,
    );
  } catch (error) {
    if (error instanceof WorkerExecutionTimeoutError) {
      records.delete(request.pluginId);
      terminateRecord(record);
    }
    throw error;
  }
}

const protocolSocket = createConnection({
  host: "127.0.0.1",
  port: protocolPort,
});
protocolSocket.setNoDelay(true);
await new Promise<void>((resolve, reject) => {
  const connected = () => {
    protocolSocket.off("error", failed);
    resolve();
  };
  const failed = (error: Error) => {
    protocolSocket.off("connect", connected);
    reject(error);
  };
  protocolSocket.once("connect", connected);
  protocolSocket.once("error", failed);
});
protocolSocket.write(`${JSON.stringify({ secret: protocolSecret })}\n`);

function writeResponse(response: Record<string, unknown>): void {
  let encoded = JSON.stringify(response);
  if (Buffer.byteLength(encoded) + 1 > MAX_PROTOCOL_BYTES) {
    encoded = JSON.stringify({
      id: response.id,
      ok: false,
      error: "Plugin response exceeds the 1 MiB protocol limit",
    });
  }
  protocolSocket.write(`${encoded}\n`);
}

async function processLine(line: string): Promise<void> {
  if (Buffer.byteLength(line) > MAX_PROTOCOL_BYTES) {
    writeResponse({ id: 0, ok: false, error: "Plugin request is too large" });
    return;
  }
  let request: HostRequest;
  try {
    request = JSON.parse(line) as HostRequest;
    if (!Number.isSafeInteger(request.id)) throw new Error("Invalid request ID");
    if (request.type === "cancel") {
      if (!Number.isSafeInteger(request.requestId) || request.requestId <= 0) {
        throw new Error("Invalid cancellation request ID");
      }
      cancelledHostRequests.add(request.requestId);
      const active = activeWorkerRequests.get(request.requestId);
      active?.record.worker.postMessage({
        type: "cancel",
        requestId: active.workerRequestId,
      });
      return;
    }
    const operation =
      request.type === "load"
        ? (loadQueue = loadQueue.then(
            () => handle(request),
            () => handle(request),
          ))
        : handle(request, (progress) => writeResponse({ id: request.id, progress }));
    const result = await operation;
    if (cancelledHostRequests.has(request.id)) {
      throw new Error("Plugin invocation cancelled");
    }
    writeResponse({ id: request.id, ok: true, result });
  } catch (error) {
    const id = typeof request! === "object" ? request!.id : 0;
    writeResponse({ id, ok: false, error: errorMessage(error) });
  } finally {
    if (typeof request! === "object" && request!.type !== "cancel") {
      cancelledHostRequests.delete(request!.id);
    }
  }
}

const lines = createInterface({ input: protocolSocket, crlfDelay: Infinity });
const activeRequests = new Set<Promise<void>>();
for await (const line of lines) {
  const request = processLine(line);
  activeRequests.add(request);
  void request.finally(() => activeRequests.delete(request));
}
await Promise.allSettled(activeRequests);
terminateAll();
