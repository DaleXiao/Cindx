import type { AgentState, AgentTraceState, ContextCheckpointView, ContextState, PersonalizationConfig, Phase3State, Phase4State, Phase5State, Phase7State, Phase8State, ProjectSessionState, ScheduleState, SidecarState, WebSearchConfigState } from "./tauriTypes";

export function newBrowserSessionId(): string {
  const randomId = globalThis.crypto?.randomUUID?.();
  const fallback = `${Date.now().toString(16)}_${Math.random().toString(16).slice(2)}`;
  return `sess_${randomId ?? fallback}`;
}

export function createBrowserContextCheckpoint(path: string | null): ContextCheckpointView {
  const now = Date.now();
  const restorePack = [
    "# Cindx Context Checkpoint",
    "",
    `- Checkpoint: browser-preview-${now}`,
    `- Generated: ${now}`,
    "- Events: 0",
    "- Tasks: 0",
    "",
    "## Current Goal",
    "- Browser preview cannot read the Rust event log. Open the Tauri app to generate a real restore pack.",
    "",
    "## Next Actions",
    "- Compact context inside the desktop app after running local agent actions.",
    ""
  ].join("\n");

  return {
    id: `browser-preview-${now}`,
    generatedAtMs: now,
    eventCount: 0,
    taskCount: 0,
    latestEventMs: now,
    currentGoal: "Open the Tauri app to generate a real restore pack.",
    completedSteps: [],
    pendingSteps: [],
    decisions: [],
    fileChanges: [],
    commandsRun: [],
    toolResults: [],
    retrievals: [],
    artifacts: [],
    errors: [],
    nextActions: ["Compact context inside the desktop app after running local agent actions."],
    path,
    restorePack
  };
}

export function createInitialPhase3State(): Phase3State {
return {
  timeline: [],
  permissions: []
};
}

export function createInitialSidecarState(): SidecarState {
return {
  browser: {
    path: "scripts/sidecars/browser-sidecar.js",
    exists: true,
    executable: true,
    healthy: true,
    healthOutput: "browser preview",
    envKey: "CINDX_BROWSER_SIDECAR"
  },
  computer: {
    path: "scripts/sidecars/computer-sidecar.js",
    exists: true,
    executable: true,
    healthy: true,
    healthOutput: "browser preview",
    envKey: "CINDX_COMPUTER_SIDECAR"
  },
  autoConfigure: true,
  lastError: null
};
}

export function createInitialWebSearchConfig(): WebSearchConfigState {
return {
  endpoint: "",
  apiKeySet: false,
  configured: false
};
}

export function createInitialPersonalizationConfig(): PersonalizationConfig {
return {
  preferredName: "",
  responseTone: "natural",
  responseLength: "balanced"
};
}

export function createInitialProjectSessionState(sessionId: string): ProjectSessionState {
return {
  projects: [
    {
      id: "project-cindx",
      name: "Cindx",
      root: ".",
      detail: "current workspace",
      status: "Selected",
      active: true,
      createdAtMs: 0,
      updatedAtMs: 0
    }
  ],
  sessions: [
    {
      id: sessionId,
      projectId: "project-cindx",
      name: "Runtime Session",
      detail: "timeline + chat",
      effort: "default",
      agentModel: "",
      status: "Active",
      titleState: "manual",
      activity: "idle",
      attentionReason: null,
      unseenResult: false,
      latestSequence: 0,
      active: true,
      archived: false,
      archivedAtMs: null,
      createdAtMs: 0,
      updatedAtMs: 0
    }
  ],
  activeProjectId: "project-cindx",
  activeSessionId: sessionId,
  lastError: null
};
}

export function createInitialScheduleState(): ScheduleState {
return {
  schedules: [],
  lastError: null
};
}

export function createInitialPhase4State(): Phase4State {
return {
  provider: {
    providerId: "openai",
    providerResource: "",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4.1",
    conductorModel: "gpt-4.1",
    plannerModel: "gpt-4.1",
    executorModel: "gpt-4.1",
    reviewerModel: "gpt-4.1",
    summarizerModel: "gpt-4.1-mini",
    fastModel: "gpt-4.1-mini",
    defaultModel: "gpt-4.1",
    highModel: "gpt-4.1",
    xhighModel: "gpt-4.1",
    embeddingModel: "text-embedding-3-large",
    imageModel: "gpt-image-2",
    imageEndpoint: "",
    voiceModel: "gpt-realtime-2.1",
    collaborationPolicy: "auto_router",
    directJudgeFailClosed: false,
    guardianAutoApproval: false,
    planFirstEnabled: false,
    approvalPolicy: "strict",
    contextWindowTokens: 1047576,
    agentSystemPrompt:
      "You are Cindx, a desktop-first assistant. Work carefully, be direct, and ask for clarification when the task is ambiguous.",
    ready: false, apiKeySet: false, authVerified: false, authVerifiedAtMs: null,
    enabledModels: []
  },
  timeline: [],
  messages: [],
  lastError: null
};
}

export function createInitialPhase5State(): Phase5State {
return {
  timeline: [],
  tools: [
    {
      name: "file.read",
      description: "Read a UTF-8 file inside the workspace.",
      risk: "read_only",
      inputSchema: "path=README.md"
    },
    {
      name: "file.list",
      description: "List files and directories inside the workspace.",
      risk: "read_only",
      inputSchema: "path=."
    },
    {
      name: "file.search",
      description: "Search UTF-8 files inside the workspace for a literal query.",
      risk: "read_only",
      inputSchema: "path=.\nquery=Phase"
    },
    {
      name: "file.write",
      description: "Write UTF-8 content to a file inside the workspace.",
      risk: "writes_workspace",
      inputSchema: "path=.cindx/demo.txt\ncontent=hello"
    },
    {
      name: "shell.run",
      description: "Run a shell command in the workspace.",
      risk: "executes_process",
      inputSchema: "command=pwd"
    },
    {
      name: "web.search",
      description: "Search the web through a lightweight HTML endpoint.",
      risk: "uses_network",
      inputSchema: "query=local agent"
    },
    {
      name: "browser.open",
      description: "Open a URL in a reusable CDP browser session.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.extract_text",
      description: "Read dynamic page text and its accessibility snapshot.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.capture",
      description: "Capture a webpage observation artifact.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.click",
      description: "Click a selector or coordinate through the browser controller.",
      risk: "uses_network",
      inputSchema: "selector=button.primary"
    },
    {
      name: "browser.type",
      description: "Type text through the browser controller.",
      risk: "sensitive_context",
      inputSchema: "selector=#q\ntext=hello"
    },
    {
      name: "browser.scroll",
      description: "Scroll the current browser page.",
      risk: "uses_network",
      inputSchema: "delta_y=600"
    },
    {
      name: "browser.tabs",
      description: "List tabs in the reusable browser session.",
      risk: "uses_network",
      inputSchema: ""
    },
    {
      name: "browser.select_tab",
      description: "Select a tab by its CDP target id.",
      risk: "uses_network",
      inputSchema: "tab_id=<target-id>"
    },
    {
      name: "computer.screenshot",
      description: "Capture a desktop screenshot artifact with redaction metadata.",
      risk: "sensitive_context",
      inputSchema: "redaction=manual"
    },
    {
      name: "computer.click",
      description: "Click a local desktop coordinate through the controller.",
      risk: "sensitive_context",
      inputSchema: "x=120\ny=240"
    },
    {
      name: "computer.type",
      description: "Type text through the controller.",
      risk: "sensitive_context",
      inputSchema: "text=hello"
    },
    {
      name: "computer.key",
      description: "Press a keyboard shortcut through the controller.",
      risk: "destructive",
      inputSchema: "key=Cmd+S\ndestructive=false"
    },
    {
      name: "computer.scroll",
      description: "Scroll through the controller.",
      risk: "sensitive_context",
      inputSchema: "delta_y=600"
    }
  ],
  pendingApprovals: [],
  results: [],
  lastError: null
};
}

export function createInitialPhase7State(): Phase7State {
return {
  timeline: [],
  stats: {
    filesIndexed: 0,
    chunksIndexed: 0,
    indexedAtMs: 0
  },
  memory: {
    records: 0,
    requirements: 0,
    outcomes: 0,
    evidence: 0,
    recalls: 0,
    observedUses: 0,
    updatedAtMs: 0
  },
  sources: [],
  retrievalTrace: null,
  graph: {
    totalNodes: 0,
    totalEdges: 0,
    nodes: [],
    edges: []
  },
  answer: null,
  lastError: null
};
}

export function createInitialPhase8State(): Phase8State {
return {
  timeline: [],
  pendingApprovals: [],
  observations: [],
  lastError: null
};
}

export function createInitialContextState(): ContextState {
return {
  timeline: [],
  checkpoint: createBrowserContextCheckpoint(null),
  lastError: null
};
}

export function createInitialAgentState(sessionId: string): AgentState {
return {
  taskId: "phase-16-agent-loop",
  projectId: "project-cindx",
  projectName: "Cindx",
  sessionId: sessionId,
  sessionName: "Runtime Session",
  status: "idle",
  turnCount: 0,
  maxTurns: 0,
  transcriptMessages: 0,
  contextTokensUsed: 0,
  contextWindowTokens: 128000,
  contextRemainingPercent: 100,
  contextUsageEstimated: true,
  runStartedAtMs: 0,
  runBudgetMs: 0,
  runModelCallBudget: 0,
  runToolCallBudget: 0,
  canCancel: false,
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
  latestAnswer: null,
  lastError: null
};
}

export function createInitialAgentTraceState(sessionId: string): AgentTraceState {
return {
  taskId: "phase-16-agent-loop",
  traceId: "browser-preview",
  runId: "browser-preview",
  projectId: "project-cindx",
  projectName: "Cindx",
  sessionId: sessionId,
  sessionName: "Runtime Session",
  status: "idle",
  startedAtMs: 0,
  finishedAtMs: null,
  durationMs: null,
  turnCount: 0,
  stepCount: 0,
  toolCallCount: 0,
  permissionWaitCount: 0,
  errorCount: 0,
  roleSummaries: [],
  exportPath: null,
  turns: [],
  lastError: null
};
}
