import { invoke } from "@tauri-apps/api/core";
import type { AgentAttachment } from "./tauriTypes";

export async function stageAgentAttachments(
  sessionId: string,
  files: File[]
): Promise<AgentAttachment[]> {
  if (files.length === 0) return [];
  if (files.length > 10) {
    throw new Error("A message can include at most 10 attachments.");
  }
  const totalBytes = files.reduce((total, file) => total + file.size, 0);
  const batchFileSizes = files.map((file) => file.size);
  const randomId = globalThis.crypto?.randomUUID?.();
  const batchId = `attachment-batch-${randomId ?? `${Date.now()}-${Math.random()}`}`;
  if (totalBytes > 50 * 1024 * 1024) {
    throw new Error("Attachments exceed the 50 MB message limit.");
  }
  const oversized = files.find((file) => file.size > 20 * 1024 * 1024);
  if (oversized) {
    throw new Error(`${oversized.name} exceeds the 20 MB attachment limit.`);
  }
  const staged: AgentAttachment[] = [];
  try {
    for (const [batchIndex, file] of files.entries()) {
      const metadata = JSON.stringify({
        sessionId,
        batchId,
        name: file.name,
        mimeType: file.type || "application/octet-stream",
        batchFileCount: files.length,
        batchFileSizes,
        batchIndex,
        batchTotalBytes: totalBytes
      });
      const metadataBytes = new TextEncoder().encode(metadata);
      const metadataHeader = btoa(String.fromCharCode(...metadataBytes));
      const bytes = new Uint8Array(await file.arrayBuffer());
      staged.push(
        await invoke<AgentAttachment>("stage_agent_attachment", bytes, {
          headers: { "x-cindx-attachment-metadata": metadataHeader }
        })
      );
    }
  } catch (error) {
    await invoke<void>("abort_agent_attachment_batch", {
      input: { sessionId, batchId }
    }).catch(async () => {
      await Promise.allSettled(
        staged.map((attachment) =>
          invoke<void>("remove_agent_attachment", {
            input: { sessionId, path: attachment.path }
          })
        )
      );
    });
    throw error;
  }
  return staged;
}
