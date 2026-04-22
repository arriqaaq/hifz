<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getCommits, getCommitDiff } from '$lib/api';
  import type { Commit } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let commit = $state<Commit | null>(null);
  let diff = $state('');
  let diffLoading = $state(false);
  let loading = $state(true);
  let error = $state('');

  let sha = $derived(decodeURIComponent(page.params.sha ?? ''));

  onMount(async () => {
    try {
      const res = await getCommits(undefined, 1, undefined, sha);
      commit = res.commits[0] ?? null;
      if (!commit) {
        error = 'Commit not found';
        return;
      }
      diffLoading = true;
      const d = await getCommitDiff(sha).catch(() => null);
      if (d?.diff) diff = d.diff;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
      diffLoading = false;
    }
  });

  function projectName(path: string): string {
    return path.split('/').pop() ?? path;
  }

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleString([], {
      month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'
    });
  }

  function sessionIdShort(sid: string | null): string {
    if (!sid) return '';
    return sid.replace(/^session:/, '');
  }

  function authorName(raw: string): string {
    const match = raw.match(/^(.+?)\s*<(.+)>$/);
    return match ? match[1] : raw;
  }

  function authorEmail(raw: string): string | null {
    const match = raw.match(/<(.+)>/);
    return match ? match[1] : null;
  }

  interface DiffFile {
    filename: string;
    lines: DiffLine[];
  }
  interface DiffLine {
    type: 'add' | 'del' | 'ctx' | 'hunk';
    text: string;
  }

  function parseDiff(raw: string): DiffFile[] {
    const files: DiffFile[] = [];
    let current: DiffFile | null = null;

    for (const line of raw.split('\n')) {
      if (line.startsWith('diff --git')) {
        const match = line.match(/b\/(.+)$/);
        current = { filename: match?.[1] ?? '', lines: [] };
        files.push(current);
      } else if (current) {
        if (line.startsWith('@@')) {
          current.lines.push({ type: 'hunk', text: line });
        } else if (line.startsWith('+') && !line.startsWith('+++')) {
          current.lines.push({ type: 'add', text: line });
        } else if (line.startsWith('-') && !line.startsWith('---')) {
          current.lines.push({ type: 'del', text: line });
        } else if (line.startsWith(' ')) {
          current.lines.push({ type: 'ctx', text: line });
        }
      }
    }
    return files;
  }

  let diffFiles = $derived(diff ? parseDiff(diff) : []);
</script>

<a href="/" class="back-link">&larr; Dashboard</a>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <p style="color: var(--accent); margin: 0;">{error}</p>
  </div>
{:else if commit}
  <div class="commit-header">
    <div class="sha-row">
      <span class="sha-label">Commit</span>
      <span class="sha-full">{commit.sha}</span>
    </div>
    <h2 class="commit-message">{commit.message}</h2>
  </div>

  <div class="meta-grid">
    <div class="meta-item">
      <span class="meta-label">Author</span>
      <span class="meta-value">
        {#if commit.author}
          <span class="author-name">{authorName(commit.author)}</span>
          {#if authorEmail(commit.author)}
            <span class="author-email">{authorEmail(commit.author)}</span>
          {/if}
        {:else}
          —
        {/if}
      </span>
    </div>
    <div class="meta-item">
      <span class="meta-label">Branch</span>
      <span class="meta-value"><span class="badge badge-blue">{commit.branch}</span></span>
    </div>
    <div class="meta-item">
      <span class="meta-label">Project</span>
      <span class="meta-value">{projectName(commit.project)}</span>
    </div>
    <div class="meta-item">
      <span class="meta-label">Date</span>
      <span class="meta-value mono">{formatDate(commit.timestamp)}</span>
    </div>
    {#if commit.insertions != null || commit.deletions != null}
      <div class="meta-item">
        <span class="meta-label">Changes</span>
        <span class="meta-value">
          {#if commit.insertions != null}<span class="ins">+{commit.insertions}</span>{/if}
          {#if commit.deletions != null}<span class="del">-{commit.deletions}</span>{/if}
        </span>
      </div>
    {/if}
    {#if commit.session_id}
      <div class="meta-item">
        <span class="meta-label">Session</span>
        <span class="meta-value">
          <a href="/sessions/{commit.session_id}" class="session-link">{sessionIdShort(commit.session_id)}</a>
        </span>
      </div>
    {/if}
  </div>

  {#if diffLoading}
    <LoadingSpinner />
  {:else if diffFiles.length > 0}
    {#each diffFiles as file}
      <div class="diff-file">
        <div class="diff-file-header">{file.filename}</div>
        <div class="diff-body">
          {#each file.lines as line}
            <div class="diff-line diff-{line.type}">{line.text}</div>
          {/each}
        </div>
      </div>
    {/each}
  {:else if commit.files_changed.length > 0}
    <div class="card">
      <div class="card-title">Files Changed ({commit.files_changed.length})</div>
      <ul class="file-list">
        {#each commit.files_changed as f}
          <li class="file-item">{f}</li>
        {/each}
      </ul>
    </div>
  {/if}
{/if}

<style>
  .back-link {
    display: inline-block;
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-ui);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-muted);
    margin-bottom: 16px;
    transition: color 150ms;
  }
  .back-link:hover { color: var(--accent); }

  .commit-header { margin-bottom: 24px; }

  .sha-row { margin-bottom: 8px; }
  .sha-label {
    display: block;
    font-size: 9px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--ink-muted);
    margin-bottom: 4px;
  }
  .sha-full {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--ink-faint);
    user-select: all;
  }

  .commit-message {
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 700;
    margin: 0;
    line-height: 1.3;
  }

  .meta-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    border: 1px solid var(--border);
    margin-bottom: 24px;
  }
  .meta-item {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-light);
    border-right: 1px solid var(--border-light);
  }
  .meta-label {
    display: block;
    font-size: 9px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-muted);
    margin-bottom: 4px;
  }
  .meta-value { font-size: 14px; font-weight: 500; }

  .mono { font-family: var(--font-mono); font-size: 12px; }

  .author-name { font-weight: 600; }
  .author-email {
    display: block;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
    margin-top: 2px;
  }

  .ins { color: #2D6A4F; font-family: var(--font-mono); font-size: 13px; margin-right: 8px; }
  .del { color: var(--accent); font-family: var(--font-mono); font-size: 13px; }

  .session-link {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
  }
  .session-link:hover { color: var(--accent); }

  .file-list { list-style: none; padding: 0; margin: 0; }
  .file-item {
    padding: 6px 0;
    border-bottom: 1px solid var(--border-light);
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--ink-secondary);
  }
  .file-item:last-child { border-bottom: none; }

  .diff-file {
    border: 1px solid var(--border);
    margin-bottom: 16px;
    overflow: hidden;
  }
  .diff-file-header {
    padding: 8px 14px;
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 600;
    background: var(--bg-alt, #F0F0EC);
    border-bottom: 1px solid var(--border);
    color: var(--ink-secondary);
  }
  .diff-body {
    overflow-x: auto;
  }
  .diff-line {
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.6;
    padding: 0 14px;
    white-space: pre;
  }
  .diff-add {
    background: #e6ffec;
    color: #1a7f37;
  }
  .diff-del {
    background: #ffebe9;
    color: #cf222e;
  }
  .diff-ctx {
    color: var(--ink-secondary);
  }
  .diff-hunk {
    background: var(--bg-alt, #F0F0EC);
    color: var(--ink-faint);
    padding: 4px 14px;
    font-style: italic;
  }
</style>
