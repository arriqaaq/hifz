# Shared commit→observation payload builder. Sourced by post-commit-hook.sh
# (live commits) and scripts/backfill-commits.sh (git-history backfill) so
# both emit the IDENTICAL `commit_made` payload. Not executable on its own.
#
# Requires `git` + `jq` in PATH. POSIX sh.

# _hifz_build_payload <sha> [session_id] [source]
# Prints the /api/v1/agent/observe JSON for <sha> to stdout. Exits the
# function non-zero (empty stdout) if git facts can't be gathered.
_hifz_build_payload() {
  _sha="$1"
  _sid="${2:-git-post-commit}"
  _src="${3:-git-post-commit-hook}"
  [ -n "$_sha" ] || return 1

  _branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null)"
  _subject="$(git log -1 --format=%s "$_sha" 2>/dev/null)"
  _message="$(git log -1 --format=%B "$_sha" 2>/dev/null)"
  _toplevel="$(git rev-parse --show-toplevel 2>/dev/null)"
  [ -n "$_toplevel" ] || return 1

  # Author gate (computed in-repo, matching src/githook.rs::ingest_one): the
  # server only runs the semantic `commits_for` linker for locally-authored
  # commits. Without these fields the server falls back to "treat as local"
  # — which mislabels a pulled teammate's commit. Fail-open: unknown local
  # identity ⇒ authored_locally=true.
  _author_email="$(git log -1 --format=%ae "$_sha" 2>/dev/null)"
  _local_email="$(git config user.email 2>/dev/null)"
  if [ -z "$_local_email" ] || [ -z "$_author_email" ] || [ "$_author_email" = "$_local_email" ]; then
    _authored_locally=true
  else
    _authored_locally=false
  fi

  # --root so the initial commit's files are captured too.
  _namestatus="$(git diff-tree --no-commit-id --root -r --name-status "$_sha" 2>/dev/null)"
  _files_json="$(printf '%s\n' "$_namestatus" | jq -R -s '
    [ split("\n")[] | select(length>0) | split("\t") | select(length>=2)
      | { code: .[0], path: .[-1] }
      | { path: .path,
          status: ( .code[0:1] as $l
                    | if $l=="A" then "A" elif $l=="D" then "D" else "M" end ) } ]')"
  [ -n "$_files_json" ] || _files_json='[]'
  _flat_files="$(printf '%s' "$_files_json" | jq '[.[].path]')"

  _is_revert=false
  _reverts_sha=null
  case "$_subject" in
    'Revert "'*) _is_revert=true ;;
  esac
  printf '%s' "$_subject" | grep -Eiq '^(revert|rollback|roll back|undo|back ?out)\b' && _is_revert=true
  _rs="$(printf '%s\n' "$_message" | grep -Eo '^This reverts commit [0-9a-f]{7,40}' | head -1 | awk '{print $NF}')"
  if [ -n "$_rs" ]; then _is_revert=true; _reverts_sha="\"$_rs\""; fi

  _keywords="$(printf '%s' "$_subject" | jq -R '[ splits("[^A-Za-z0-9_]+") | select(length>2) ]')"
  _ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  jq -n \
    --arg sessionId "$_sid" \
    --arg project "$_toplevel" \
    --arg cwd "$_toplevel" \
    --arg ts "$_ts" \
    --arg title "commit: ${_branch}: ${_subject}" \
    --arg subject "$_subject" \
    --arg sha "$_sha" --arg branch "$_branch" --arg message "$_message" \
    --arg src "$_src" \
    --arg author_email "$_author_email" \
    --argjson authored_locally "$_authored_locally" \
    --argjson files "$_flat_files" \
    --argjson file_status "$_files_json" \
    --argjson is_revert "$_is_revert" \
    --argjson reverts_sha "$_reverts_sha" \
    --argjson keywords "$_keywords" \
    '{
       hookType: "post_tool_use", sessionId: $sessionId,
       project: $project, cwd: $cwd,
       obs_type: "commit_made", timestamp: $ts, source: $src,
       title: $title,
       facts: [ ("sha:" + $sha), ("branch:" + $branch) ],
       keywords: $keywords, files: $files, importance: 8,
       data: { tool_name: "Bash",
               tool_input: { command: ("git commit -m " + ($subject | @json)) },
               via: $src },
       metadata: { sha: $sha, branch: $branch, message: $message,
                   files: $files, file_status: $file_status,
                   is_revert: $is_revert, reverts_sha: $reverts_sha,
                   author_email: $author_email,
                   authored_locally: $authored_locally }
     }'
}
