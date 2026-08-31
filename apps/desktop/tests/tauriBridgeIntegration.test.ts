import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import {
  committedSteerReconciliation,
  committedSteerUserMessage
} from "../src/sessionRuntimeModel.ts";
import {
  providerSubmissionPreflight,
  resolveProviderReadiness
} from "../src/providerReadinessModel.ts";
import { permissionFocusTarget } from "../src/components/accessibilityFocusModel.ts";
import { ModelStreamAccumulator } from "../src/components/modelStreamAccumulator.ts";
import { ModelStreamEventRouter } from "../src/components/modelStreamEventRouter.ts";
import { startModelStreamSubscription } from "../src/components/modelStreamSubscription.ts";
import { StreamingMarkdownModel } from "../src/components/streamingMarkdownModel.ts";
import type {
  AgentState,
  ModelStreamDelta,
  PermissionReviewState,
  Phase4State,
  ProviderConfigInput,
  QueuedAgentMessage,
  QueuedAgentMessageActionReceipt,
  QueuedAgentMessageReceipt
} from "../src/tauriTypes.ts";

type InvokeCall = {
  command: string;
  args: unknown;
  options: unknown;
};

type EventRegistration = {
  event: string;
  callbackId: number;
};

const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const calls: InvokeCall[] = [];
const replies = new Map<string, unknown>();
const failures = new Map<string, unknown>();
const callbacks = new Map<number, (value: unknown) => void>();
const listeners = new Map<number, EventRegistration>();
let nextCallbackId = 1;
let nextEventId = 100;

const fakeWindow = {
  __TAURI_INTERNALS__: {
    transformCallback(callback?: (value: unknown) => void, once = false) {
      const id = nextCallbackId;
      nextCallbackId += 1;
      callbacks.set(id, (value) => {
        callback?.(value);
        if (once) callbacks.delete(id);
      });
      return id;
    },
    unregisterCallback(id: number) {
      callbacks.delete(id);
    },
    async invoke(command: string, args: unknown = {}, options?: unknown) {
      calls.push({ command, args, options });
      if (failures.has(command)) throw failures.get(command);
      if (command === "plugin:event|listen") {
        const input = args as { event: string; handler: number };
        const eventId = nextEventId;
        nextEventId += 1;
        listeners.set(eventId, { event: input.event, callbackId: input.handler });
        return eventId;
      }
      return replies.get(command);
    }
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: {
    unregisterListener(_event: string, eventId: number) {
      const listener = listeners.get(eventId);
      if (listener) callbacks.delete(listener.callbackId);
      listeners.delete(eventId);
    }
  }
};

Object.defineProperty(globalThis, "window", {
  configurable: true,
  writable: true,
  value: fakeWindow
});

const tauri = await import("../src/tauri.ts");

beforeEach(() => {
  calls.length = 0;
  replies.clear();
  failures.clear();
  callbacks.clear();
  listeners.clear();
  nextCallbackId = 1;
  nextEventId = 100;
});

after(() => {
  if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
  else delete (globalThis as { window?: unknown }).window;
});

function emit(eventName: string, payload: unknown) {
  const registration = [...listeners.entries()].find(
    ([, listener]) => listener.event === eventName
  );
  assert.ok(registration, `listener for ${eventName} should be registered`);
  const [eventId, listener] = registration;
  const callback = callbacks.get(listener.callbackId);
  assert.ok(callback, `callback ${listener.callbackId} should be registered`);
  callback({ event: eventName, id: eventId, payload });
}

function commandCalls() {
  return calls.filter((call) => !call.command.startsWith("plugin:event|"));
}

async function flushPromiseJobs() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function providerInput(): ProviderConfigInput {
  return {
    providerId: "openai",
    providerResource: "",
    baseUrl: "https://api.openai.com/v1",
    apiKey: "test-key",
    model: "gpt-test",
    conductorModel: "gpt-test",
    plannerModel: "gpt-test",
    executorModel: "gpt-test",
    reviewerModel: "gpt-test",
    summarizerModel: "gpt-test",
    embeddingModel: "text-embedding-test",
    imageModel: "image-test",
    imageEndpoint: "",
    voiceModel: "voice-test",
    collaborationPolicy: "auto_router",
    promptEvolutionEnabled: true,
    contextWindowTokens: 128000,
    agentSystemPrompt: ""
  };
}

test("queue then steer crosses the IPC boundary and reconciles exactly once", async () => {
  const message: QueuedAgentMessage = {
    id: "queue-a",
    sessionId: "session-a",
    prompt: "Use the latest result",
    attachments: [],
    effort: "pro",
    mode: "steer",
    createdAtMs: 10,
    updatedAtMs: 20
  };
  const queuedReceipt: QueuedAgentMessageReceipt = {
    message: { ...message, mode: "queue", updatedAtMs: 10 },
    eventCount: 1,
    latestSequence: 1,
    latestTimestampMs: 10
  };
  const steerReceipt: QueuedAgentMessageActionReceipt = {
    queueId: message.id,
    message,
    eventCount: 2,
    latestSequence: 2,
    latestTimestampMs: 20,
    cancelledActiveRun: true,
    steerCommitted: true
  };
  replies.set("queue_agent_message", queuedReceipt);
  replies.set("steer_queued_agent_message", steerReceipt);

  const queued = await tauri.queueAgentMessage(
    message.prompt,
    message.sessionId,
    [],
    message.effort,
    message.id
  );
  const steered = await tauri.steerQueuedAgentMessage(message.sessionId, message.id);

  assert.equal(queued, queuedReceipt);
  assert.equal(steered, steerReceipt);
  const ipcCalls = commandCalls();
  assert.equal(ipcCalls.length, 2);
  assert.equal(ipcCalls[0].command, "queue_agent_message");
  assert.equal(ipcCalls[0].options, undefined);
  const queueInput = (ipcCalls[0].args as { input: Record<string, unknown> }).input;
  assert.deepEqual(
    { ...queueInput, currentTime: "<runtime>" },
    {
      prompt: message.prompt,
      sessionId: message.sessionId,
      currentTime: "<runtime>",
      effort: message.effort,
      attachments: [],
      queueId: message.id,
      planMode: false
    }
  );
  assert.equal(typeof queueInput.currentTime, "string");
  assert.ok((queueInput.currentTime as string).includes("UTC"));
  assert.deepEqual(ipcCalls[1], {
    command: "steer_queued_agent_message",
    args: { input: { sessionId: message.sessionId, queueId: message.id } },
    options: undefined
  });
  assert.equal(committedSteerUserMessage(message, steered)?.queueId, message.id);
  assert.equal(
    committedSteerReconciliation(
      {
        status: "running",
        canCancel: true,
        messages: [{ role: "user", queueId: message.id }],
        queuedMessages: []
      } as unknown as AgentState,
      message.id
    ),
    "applied"
  );
});

test("first provider setup becomes ready without a workspace precondition", async () => {
  const input = providerInput();
  const modelLookup = {
    providerId: input.providerId,
    providerResource: input.providerResource,
    baseUrl: input.baseUrl,
    apiKey: input.apiKey
  };
  const savedState = { provider: { ready: true } } as unknown as Phase4State;
  const modelState = { models: [input.model], fetchedAtMs: 10, lastError: null };
  replies.set("save_provider_config", savedState);
  replies.set("list_provider_models", modelState);

  assert.equal(providerSubmissionPreflight("unconfigured").clearDraft, false);
  const saved = await tauri.saveProviderConfig(input);
  const models = await tauri.listProviderModels(modelLookup);

  assert.equal(resolveProviderReadiness(saved.provider, false, false), "ready");
  assert.equal(providerSubmissionPreflight("ready").allowSubmit, true);
  assert.equal(models, modelState);
  assert.deepEqual(commandCalls(), [
    {
      command: "save_provider_config",
      args: { input },
      options: undefined
    },
    {
      command: "list_provider_models",
      args: { input: modelLookup },
      options: undefined
    }
  ]);
  assert.ok(
    commandCalls().every((call) =>
      !JSON.stringify(call.args).toLowerCase().includes("workspace")
    )
  );
});

test("permission waiting resolves against the original session contract", async () => {
  const reviewState = {
    pending: [{ requestId: "permission-a", sessionId: "session-a" }]
  } as unknown as PermissionReviewState;
  const agentState = {
    taskId: "phase-16-agent-loop",
    projectId: null,
    projectName: null,
    sessionId: "session-a",
    sessionName: null,
    status: "running",
    turnCount: 0,
    maxTurns: 0,
    transcriptMessages: 0,
    contextTokensUsed: 0,
    contextWindowTokens: 0,
    contextRemainingPercent: 100,
    contextUsageEstimated: false,
    runStartedAtMs: 0,
    runBudgetMs: 0,
    runModelCallBudget: 0,
    runToolCallBudget: 0,
    canCancel: true,
    canRetry: false,
    canContinue: false,
    eventCount: 0,
    latestSequence: 0,
    oldestSequence: 0,
    hasOlderHistory: false,
    timeline: [],
    messages: [],
    pendingApprovals: [],
    queuedMessages: [],
    pendingPlanConfirmation: null,
    latestAnswer: null,
    lastError: null
  } as unknown as AgentState;
  replies.set("get_permission_review_state", reviewState);
  replies.set("resolve_agent_permission", agentState);

  const review = await tauri.getPermissionReviewState();
  assert.equal(
    permissionFocusTarget(null, review.pending[0].requestId, false),
    "request"
  );
  const resolved = await tauri.resolveAgentPermission(
    review.pending[0].requestId,
    "allow_for_session",
    "session-a"
  );

  assert.equal(resolved, agentState);
  assert.equal(permissionFocusTarget("permission-a", null, true), "composer");
  assert.deepEqual(commandCalls()[1], {
    command: "resolve_agent_permission",
    args: {
      requestId: "permission-a",
      decision: "allow_for_session",
      sessionId: "session-a",
      grantCommandPrefix: false
    },
    options: undefined
  });

  calls.length = 0;
  await tauri.resolveAgentPermission(
    review.pending[0].requestId,
    "allow_for_session",
    "session-a",
    true
  );
  assert.deepEqual(commandCalls()[0], {
    command: "resolve_agent_permission",
    args: {
      requestId: "permission-a",
      decision: "allow_for_session",
      sessionId: "session-a",
      grantCommandPrefix: true
    },
    options: undefined
  });
});

test("fork and delete failures remain failures in the native runtime", async () => {
  for (const [command, operation] of [
    ["fork_session", () => tauri.forkSession("session-a")],
    ["delete_session", () => tauri.deleteSession("session-a")]
  ] as const) {
    calls.length = 0;
    failures.clear();
    const failure = new Error(`${command} failed`);
    failures.set(command, failure);

    await assert.rejects(operation(), (error) => error === failure);
    assert.deepEqual(commandCalls(), [
      {
        command,
        args: { input: { sessionId: "session-a" } },
        options: undefined
      }
    ]);
  }
});

test("native stream events stay isolated to the selected session and unlisten cleanly", async () => {
  const snapshots: number[] = [];
  const done: string[] = [];
  const errors: string[] = [];
  const accumulator = new ModelStreamAccumulator({
    model: new StreamingMarkdownModel(),
    schedule: () => 1,
    cancel: () => {},
    onSnapshot: (snapshot) => snapshots.push(snapshot.totalLength)
  });
  const router = new ModelStreamEventRouter();
  let activeSessionId = "session-a";
  const dispose = startModelStreamSubscription({
    subscribe: tauri.subscribeToModelStream,
    router,
    accumulator,
    activeSessionId: () => activeSessionId,
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return false;
    },
    onError: (message) => errors.push(message)
  });
  await flushPromiseJobs();

  const event = (
    sessionId: string,
    requestId: string,
    overrides: Partial<ModelStreamDelta> = {}
  ): ModelStreamDelta => ({
    taskId: "phase-16-agent-loop",
    requestId,
    sessionId,
    delta: "delta",
    done: false,
    reset: false,
    error: null,
    ...overrides
  });
  emit("model-stream-delta", event("session-a", "request-a", { delta: "answer-a" }));
  assert.equal(accumulator.snapshot().totalLength, 8);

  activeSessionId = "session-b";
  router.reset();
  accumulator.reset();
  emit("model-stream-delta", event("session-a", "request-a", { delta: "late-a" }));
  emit(
    "model-stream-delta",
    event("session-a", "request-a", { delta: "", done: true })
  );
  assert.equal(accumulator.snapshot().totalLength, 0);

  emit("model-stream-delta", event("session-b", "request-b", { delta: "answer-b" }));
  emit(
    "model-stream-delta",
    event("session-b", "request-b", { delta: "", done: true })
  );
  assert.equal(accumulator.snapshot().totalLength, 8);
  assert.deepEqual(done, ["session-b"]);
  assert.deepEqual(errors, []);

  dispose();
  await flushPromiseJobs();
  assert.equal(listeners.size, 0);
  assert.equal(callbacks.size, 0);
  assert.equal(
    calls.filter((call) => call.command === "plugin:event|unlisten").length,
    1
  );
  accumulator.dispose();

  failures.set("plugin:event|listen", new Error("listen failed"));
  await assert.rejects(
    tauri.subscribeToModelStream(() => {}),
    /listen failed/
  );
});
