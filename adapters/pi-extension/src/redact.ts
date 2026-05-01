const SECRET_KEY_RE = /(token|key|secret|password|auth|bearer|api[_-]?key)/i;

/** Walk a JSON value; replace string values whose key matches SECRET_KEY_RE with "[redacted]". */
export function redact<T>(value: T): T {
  return walk(value) as T;
}

function walk(v: unknown): unknown {
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) return v.map(walk);
  const out: Record<string, unknown> = {};
  for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
    if (SECRET_KEY_RE.test(k) && typeof val === "string") {
      out[k] = "[redacted]";
    } else {
      out[k] = walk(val);
    }
  }
  return out;
}
