import { LoaderCircle, Mic } from "lucide-react";
import { useVoiceInput } from "../voice/useVoiceInput";
import {
  voiceInputButtonDisabled,
  type VoiceInputStatus
} from "../voice/voiceInputModel";
import type { ProviderVoiceTransport } from "../providerProfiles";

type VoiceInputButtonProps = {
  configured: boolean;
  transport: ProviderVoiceTransport;
  sessionId: string | null;
  onTranscript: (sessionId: string, text: string) => void;
  onError: (message: string) => void;
  onStatusChange: (status: VoiceInputStatus) => void;
};

export function VoiceInputButton({
  configured,
  transport,
  sessionId,
  onTranscript,
  onError,
  onStatusChange
}: VoiceInputButtonProps) {
  const { canvasRef, status, toggle } = useVoiceInput({
    enabled: configured,
    transport,
    sessionId,
    onTranscript,
    onError,
    onStatusChange
  });
  const active = status !== "idle";
  const disabled = voiceInputButtonDisabled(configured, transport, sessionId, status);
  const title = !configured
    ? "Configure a full-duplex voice model in Settings"
    : !sessionId
      ? "Open a session to use voice input"
      : status === "finishing"
        ? "Finishing voice input"
        : active
          ? "Stop voice input"
          : "Start voice input";

  return (
    <button
      className="composer-voice-button"
      type="button"
      data-status={status}
      aria-label={title}
      aria-pressed={active}
      title={title}
      disabled={disabled}
      onClick={toggle}
    >
      <canvas ref={canvasRef} width={64} height={64} aria-hidden="true" />
      {status === "connecting" || status === "finishing" ? (
        <LoaderCircle className="composer-voice-loading" aria-hidden="true" />
      ) : status === "idle" ? (
        <Mic aria-hidden="true" />
      ) : null}
    </button>
  );
}
