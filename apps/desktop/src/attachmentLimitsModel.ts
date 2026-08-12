export const ATTACHMENT_MAX_FILES = 10;
export const ATTACHMENT_MAX_FILE_BYTES = 20 * 1024 * 1024;
export const ATTACHMENT_MAX_TOTAL_BYTES = 50 * 1024 * 1024;

export interface AttachmentBatchCandidate {
  name: string;
  size: number;
}

export function attachmentBatchTotalBytes(files: ReadonlyArray<AttachmentBatchCandidate>): number {
  return files.reduce((total, file) => total + file.size, 0);
}

export function validateAttachmentBatch(
  files: ReadonlyArray<AttachmentBatchCandidate>
): string | null {
  if (files.length > ATTACHMENT_MAX_FILES) {
    return "A message can include at most 10 attachments.";
  }
  if (attachmentBatchTotalBytes(files) > ATTACHMENT_MAX_TOTAL_BYTES) {
    return "Attachments exceed the 50 MB message limit.";
  }
  const oversized = files.find((file) => file.size > ATTACHMENT_MAX_FILE_BYTES);
  if (oversized) {
    return `${oversized.name} exceeds the 20 MB attachment limit.`;
  }
  return null;
}
