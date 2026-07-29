import assert from "node:assert/strict";
import test from "node:test";
import {
  MAX_VOICE_TRANSCRIPT_CHARS,
  appendVoiceTranscript,
  parseVoiceServerEvent,
  reduceVoiceTurnEvent,
  voiceStopAction,
  type VoiceTurnState
} from "../src/voice/voiceInputModel.ts";

test("voice events expose completed input transcripts but never interim deltas", () => {
  assert.equal(
    parseVoiceServerEvent(
      JSON.stringify({
        type: "conversation.item.input_audio_transcription.delta",
        item_id: "item-1",
        delta: "uncorrected interim"
      })
    ),
    null
  );
  assert.deepEqual(
    parseVoiceServerEvent(
      JSON.stringify({
        type: "conversation.item.input_audio_transcription.completed",
        item_id: "item-1",
        transcript: "Corrected final text"
      })
    ),
    { kind: "transcript", itemId: "item-1", text: "Corrected final text" }
  );
});

test("voice events reject malformed and unrelated payloads while preserving service errors", () => {
  assert.equal(parseVoiceServerEvent("not-json"), null);
  assert.equal(parseVoiceServerEvent(JSON.stringify({ type: "response.text.delta" })), null);
  assert.deepEqual(
    parseVoiceServerEvent(
      JSON.stringify({ type: "input_audio_buffer.committed", item_id: "item-1" })
    ),
    { kind: "committed", itemId: "item-1" }
  );
  assert.deepEqual(
    parseVoiceServerEvent(JSON.stringify({ type: "error", error: { message: "bad audio" } })),
    { kind: "error", message: "bad audio" }
  );
});

test("voice stop cancels connection setup but gives recorded audio a finishing phase", () => {
  assert.equal(voiceStopAction("connecting"), "discard");
  assert.equal(voiceStopAction("recording"), "finish");
  assert.equal(voiceStopAction("finishing"), "none");
  assert.equal(voiceStopAction("idle"), "none");
});

test("finishing accepts this commit once and ignores mismatched or late transcripts", () => {
  const finishing: VoiceTurnState = {
    status: "finishing",
    commitSent: true,
    committedItemId: null,
    seenItemIds: []
  };
  const committed = reduceVoiceTurnEvent(finishing, {
    kind: "committed",
    itemId: "item-current"
  }).state;
  assert.equal(
    reduceVoiceTurnEvent(committed, {
      kind: "transcript",
      itemId: "item-old",
      text: "stale"
    }).completed,
    false
  );
  const completed = reduceVoiceTurnEvent(committed, {
    kind: "transcript",
    itemId: "item-current",
    text: "final"
  });
  assert.equal(completed.completed, true);
  assert.equal(completed.transcript, "final");
  assert.equal(
    reduceVoiceTurnEvent(completed.state, {
      kind: "transcript",
      itemId: "item-current",
      text: "duplicate"
    }).completed,
    false
  );
  assert.equal(
    reduceVoiceTurnEvent(
      { ...finishing, status: "idle" },
      { kind: "transcript", itemId: "item-current", text: "late" }
    ).completed,
    false
  );
});

test("empty completed transcripts still finish the turn without appending text", () => {
  assert.deepEqual(
    parseVoiceServerEvent(
      JSON.stringify({
        type: "conversation.item.input_audio_transcription.completed",
        item_id: "item-silent",
        transcript: "   "
      })
    ),
    { kind: "transcript", itemId: "item-silent", text: "" }
  );
});

test("completed transcripts append with natural word boundaries", () => {
  assert.equal(appendVoiceTranscript("Existing", "spoken text"), "Existing spoken text");
  assert.equal(appendVoiceTranscript("已有", "语音输入"), "已有语音输入");
  assert.equal(appendVoiceTranscript("Hello", ", world"), "Hello, world");
});

test("voice transcript text is bounded before it reaches composer state", () => {
  const parsed = parseVoiceServerEvent(
    JSON.stringify({
      type: "conversation.item.input_audio_transcription.completed",
      item_id: "item-1",
      transcript: "x".repeat(MAX_VOICE_TRANSCRIPT_CHARS + 100)
    })
  );
  assert.equal(parsed?.kind === "transcript" ? parsed.text.length : 0, MAX_VOICE_TRANSCRIPT_CHARS);
});
