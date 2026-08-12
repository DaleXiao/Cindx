import { invoke } from "@tauri-apps/api/core";
import { attachmentBatchTotalBytes, validateAttachmentBatch } from "./attachmentLimitsModel.ts";
import type { AgentAttachment } from "./tauriTypes";

export async function stageAgentAttachments(
  sessionId: string,
  files: File[]
): Promise<AgentAttachment[]> {
  if (files.length === 0) return [];
  const batchFileSizes = files.map((file) => file.size);
  const totalBytes = attachmentBatchTotalBytes(files);
  const validationError = validateAttachmentBatch(files);
  if (validationError) {
    throw new Error(validationError);
  }
  const randomId = globalThis.crypto?.randomUUID?.();
  const batchId = `attachment-batch-${randomId ?? `${Date.now()}-${Math.random()}`}`;
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
