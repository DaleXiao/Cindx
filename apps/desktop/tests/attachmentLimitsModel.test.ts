import assert from "node:assert/strict";
import test from "node:test";
import {
  ATTACHMENT_MAX_FILE_BYTES,
  ATTACHMENT_MAX_FILES,
  ATTACHMENT_MAX_TOTAL_BYTES,
  attachmentBatchTotalBytes,
  validateAttachmentBatch
} from "../src/attachmentLimitsModel.ts";

const MB = 1024 * 1024;

function file(name: string, size: number) {
  return { name, size };
}

test("an empty batch is valid", () => {
  assert.equal(validateAttachmentBatch([]), null);
});

test("up to ten files are accepted and eleven are rejected", () => {
  const ten = Array.from({ length: ATTACHMENT_MAX_FILES }, (_, index) => file(`f${index}.txt`, 10));
  assert.equal(validateAttachmentBatch(ten), null);
  const eleven = [...ten, file("extra.txt", 10)];
  assert.equal(
    validateAttachmentBatch(eleven),
    "A message can include at most 10 attachments."
  );
});

test("a total at the 50 MB boundary passes and anything above fails", () => {
  const atLimit = [
    file("a.bin", 19 * MB),
    file("b.bin", 19 * MB),
    file("c.bin", ATTACHMENT_MAX_TOTAL_BYTES - 38 * MB)
  ];
  assert.equal(attachmentBatchTotalBytes(atLimit), ATTACHMENT_MAX_TOTAL_BYTES);
  assert.equal(validateAttachmentBatch(atLimit), null);
  const over = [
    file("a.bin", 19 * MB),
    file("b.bin", 19 * MB),
    file("c.bin", ATTACHMENT_MAX_TOTAL_BYTES - 38 * MB + 1)
  ];
  assert.equal(
    validateAttachmentBatch(over),
    "Attachments exceed the 50 MB message limit."
  );
});

test("a single file at the 20 MB boundary passes and above fails with its name", () => {
  assert.equal(validateAttachmentBatch([file("ok.bin", ATTACHMENT_MAX_FILE_BYTES)]), null);
  assert.equal(
    validateAttachmentBatch([file("big.bin", ATTACHMENT_MAX_FILE_BYTES + 1)]),
    "big.bin exceeds the 20 MB attachment limit."
  );
});

test("the total limit is reported before the per-file limit", () => {
  const single = [file("huge.bin", 60 * MB)];
  assert.equal(
    validateAttachmentBatch(single),
    "Attachments exceed the 50 MB message limit."
  );
});

test("attachmentBatchTotalBytes sums the batch", () => {
  assert.equal(attachmentBatchTotalBytes([file("a", 1), file("b", 2), file("c", 3)]), 6);
  assert.equal(attachmentBatchTotalBytes([]), 0);
});
