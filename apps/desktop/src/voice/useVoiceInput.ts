import { useRef } from "react";
import type { ProviderVoiceTransport } from "../providerProfiles";
import type { VoiceInputStatus } from "./voiceInputModel";
import { useAlibabaVoiceInput } from "./useAlibabaVoiceInput";
import { useOpenAiVoiceInput } from "./useOpenAiVoiceInput";

type UseVoiceInputOptions = {
  enabled: boolean;
  transport: ProviderVoiceTransport;
  sessionId: string | null;
  onTranscript: (sessionId: string, text: string) => void;
  onError: (message: string) => void;
  onStatusChange: (status: VoiceInputStatus) => void;
};

export function useVoiceInput(options: UseVoiceInputOptions) {
  const unavailableCanvasRef = useRef<HTMLCanvasElement>(null);
  const openAi = useOpenAiVoiceInput({
    ...options,
    enabled: options.enabled && options.transport === "openai_webrtc"
  });
  const alibaba = useAlibabaVoiceInput({
    ...options,
    enabled: options.enabled && options.transport === "dashscope_websocket"
  });
  if (options.transport === "openai_webrtc") return openAi;
  if (options.transport === "dashscope_websocket") return alibaba;
  return {
    canvasRef: unavailableCanvasRef,
    status: "idle" as const,
    toggle: () => {}
  };
}
