const MAX_VOICE_EVENT_CHARS = 256 * 1024;
export const MAX_VOICE_TRANSCRIPT_CHARS = 16_000;

export type VoiceInputStatus = "idle" | "connecting" | "recording" | "finishing";

export type VoiceServerEvent =
  | { kind: "transcript"; itemId: string; text: string }
  | { kind: "committed"; itemId: string }
  | { kind: "error"; message: string };

export type VoiceTurnState = {
  status: VoiceInputStatus;
  commitSent: boolean;
  committedItemId: string | null;
  seenItemIds: readonly string[];
};

export type VoiceTurnResult = {
  state: VoiceTurnState;
  completed: boolean;
  transcript: string;
};

export function voiceStopAction(status: VoiceInputStatus): "discard" | "finish" | "none" {
  if (status === "connecting") return "discard";
  if (status === "recording") return "finish";
  return "none";
}

export function reduceVoiceTurnEvent(
  state: VoiceTurnState,
  event: VoiceServerEvent
): VoiceTurnResult {
  if (event.kind === "committed") {
    const next = state.status === "finishing" && state.commitSent && !state.committedItemId
      ? { ...state, committedItemId: event.itemId }
      : state;
    return { state: next, completed: false, transcript: "" };
  }
  if (
    event.kind !== "transcript" ||
    state.status !== "finishing" ||
    !state.commitSent ||
    state.seenItemIds.includes(event.itemId) ||
    (state.committedItemId !== null && state.committedItemId !== event.itemId)
  ) {
    return { state, completed: false, transcript: "" };
  }
  return {
    state: { ...state, seenItemIds: [...state.seenItemIds, event.itemId] },
    completed: true,
    transcript: event.text
  };
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function cleanText(value: unknown, maxChars: number): string {
  if (typeof value !== "string") return "";
  return value
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "")
    .slice(0, maxChars);
}

export function parseVoiceServerEvent(payload: string): VoiceServerEvent | null {
  if (!payload || payload.length > MAX_VOICE_EVENT_CHARS) return null;
  let value: Record<string, unknown> | null;
  try {
    value = record(JSON.parse(payload));
  } catch {
    return null;
  }
  if (!value || typeof value.type !== "string") return null;
  if (value.type === "error" || value.type === "conversation.item.input_audio_transcription.failed") {
    const nested = record(value.error);
    const message = cleanText(nested?.message ?? value.message, 500).trim();
    return { kind: "error", message: message || "Voice service reported an error" };
  }
  if (value.type === "input_audio_buffer.committed") {
    const itemId = cleanText(value.item_id, 256).trim();
    return itemId ? { kind: "committed", itemId } : null;
  }
  if (value.type !== "conversation.item.input_audio_transcription.completed") return null;
  const itemId = cleanText(value.item_id, 256).trim();
  const text = cleanText(value.transcript, MAX_VOICE_TRANSCRIPT_CHARS).trim();
  return itemId ? { kind: "transcript", itemId, text } : null;
}

export function appendVoiceTranscript(draft: string, transcript: string): string {
  const text = transcript.trim();
  if (!text) return draft;
  if (!draft || /\s$/u.test(draft)) return draft + text;
  const tightJoin = /[\u3400-\u9fff]$/u.test(draft) || /^[\u3400-\u9fff，。！？、；：,.!?;:)]/u.test(text);
  return `${draft}${tightJoin ? "" : " "}${text}`;
}
