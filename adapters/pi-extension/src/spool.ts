import { mkdirSync, appendFileSync, readdirSync, readFileSync, statSync, unlinkSync } from "node:fs";
import { join } from "node:path";

/**
 * Append-only on-disk fallback for events that failed to POST.
 * Replayed at startup and after a successful HTTP recovery.
 *
 * One file per UTC day; rotated at 100 MB; no replay across producer renames.
 */
const MAX_FILE_BYTES = 100 * 1024 * 1024;

export class Spool {
  private dir: string;

  constructor(dir: string) {
    this.dir = dir;
    mkdirSync(this.dir, { recursive: true });
  }

  append(kind: "event" | "observation" | "memory" | "consolidate" | "session", body: unknown): void {
    const file = this.activeFile();
    const line = JSON.stringify({ kind, body, ts: Date.now() }) + "\n";
    appendFileSync(file, line, "utf8");
  }

  /** Returns each spooled record in append order; caller is responsible for re-POSTing. */
  *drain(): Generator<{ file: string; line: string }> {
    const files = readdirSync(this.dir).filter((f) => f.endsWith(".jsonl")).sort();
    for (const f of files) {
      const full = join(this.dir, f);
      const text = readFileSync(full, "utf8");
      for (const line of text.split("\n")) {
        if (line) yield { file: full, line };
      }
    }
  }

  /** Remove a fully-drained spool file. */
  remove(file: string): void {
    try {
      unlinkSync(file);
    } catch {
      // ignore — file may already be gone (concurrent drain)
    }
  }

  private activeFile(): string {
    const day = new Date().toISOString().slice(0, 10); // YYYY-MM-DD
    let i = 0;
    let file = join(this.dir, `${day}.jsonl`);
    // Rotate within the same day if we exceed the size cap.
    while (sizeOf(file) >= MAX_FILE_BYTES) {
      i++;
      file = join(this.dir, `${day}-${i}.jsonl`);
    }
    return file;
  }
}

function sizeOf(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}
