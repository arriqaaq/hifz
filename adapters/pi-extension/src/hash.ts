import { createHash } from "node:crypto";

/**
 * Deterministic event hash. Inputs are chosen so the result is unique-by-construction:
 *   - tool events:      (sessionId, toolCallId, eventType)
 *   - lifecycle/other:  (sessionId, eventType, sequence)
 * The Hifz `event_hash` UNIQUE index then makes retried POSTs idempotent.
 */
export function hashEvent(
  sessionId: string,
  eventType: string,
  sequence: number,
  payload: unknown,
): string {
  const h = createHash("sha256");
  h.update(sessionId);
  h.update("\0");
  h.update(eventType);
  h.update("\0");
  // Tool events carry a stable toolCallId; prefer it over sequence for natural idempotency.
  const tcid = extractToolCallId(payload);
  if (tcid) {
    h.update(tcid);
  } else {
    h.update(String(sequence));
  }
  return h.digest("hex");
}

function extractToolCallId(p: unknown): string | undefined {
  if (!p || typeof p !== "object") return undefined;
  const o = p as Record<string, unknown>;
  if (typeof o.toolCallId === "string") return o.toolCallId;
  return undefined;
}
