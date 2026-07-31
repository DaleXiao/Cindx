import type { AgentRunBudget, AgentRunBudgets } from "./agentRunBudgetModel.ts";
import type {
  AgentHistoryPage,
  AgentState,
  AgentStateDelta,
  ChatMessageView,
  TimelineEntry
} from "./tauriTypes.ts";

export type NativeAgentRunBudgetView = AgentRunBudget;
export type NativeAgentRunBudgetsView = AgentRunBudgets;

export type NativeRuntimeStatus = {
  appVersion: string;
  kernelStatus: string;
  providerReady: boolean;
  workspaceRoot: string;
  orchestrationModes: string[];
  registeredTools: string[];
  agentRunBudgets: AgentRunBudgets;
};

export type NativeTimelineEntry = TimelineEntry & {
  sequence: number;
};

export type NativeChatMessageView = ChatMessageView & {
  sequence: number;
  runId: string | null;
  queueId: string | null;
  attachments: NonNullable<ChatMessageView["attachments"]>;
};

export type NativeAgentState = Omit<AgentState, "timeline" | "messages"> & {
  timeline: NativeTimelineEntry[];
  messages: NativeChatMessageView[];
};

export type NativeAgentStateDelta = Omit<AgentStateDelta, "state"> & {
  state: NativeAgentState;
};

export type NativeAgentHistoryPage = Omit<AgentHistoryPage, "timeline" | "messages"> & {
  timeline: NativeTimelineEntry[];
  messages: NativeChatMessageView[];
};
