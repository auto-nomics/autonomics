#!/usr/bin/env bash

# Run a non-interactive Codex task at a specified local time.
#
# Examples:
#   scripts/codex_task.sh --at "09:30" -- "请总结这个仓库的构建方式"
#   scripts/codex_task.sh --at "2026-08-16 09:30" --prompt-file task.txt
#   scripts/codex_task.sh -- "请运行测试"     # for use from cron
#
# A time containing only HH:MM[:SS] is scheduled today, or tomorrow if that
# time has already passed. Full dates are interpreted in the local timezone.

set -Eeuo pipefail

usage() {
  cat <<'EOF'
Usage:
  codex_task.sh [options] -- "PROMPT"
  codex_task.sh [options] --prompt-file FILE

Options:
  -a, --at TIME           Target local time: HH:MM[:SS] or YYYY-MM-DD HH:MM[:SS].
                          If omitted, the task starts immediately.
  -C, --workdir DIR       Codex working directory. Default: script's project root.
  -m, --model NAME        Model passed to Codex.
  -s, --sandbox MODE      read-only, workspace-write, or yolo.
                          Default: workspace-write.
  -t, --timeout SECONDS   Stop Codex after this many seconds. Default: 3600.
  -l, --log-file FILE     Append full output to FILE.
                          Default: ~/.codex/scheduled-task-logs/<timestamp>.log
  -f, --prompt-file FILE  Read the task prompt from FILE. Use "-" for stdin.
      --codex-arg ARG     Append an extra argument to `codex exec`. Repeatable.
      --dry-run           Print the command without waiting or running it.
  -h, --help              Show this help.
EOF
}

die() {
  printf 'codex_task.sh: %s\n' "$*" >&2
  exit 1
}

need_value() {
  [[ $# -ge 2 ]] || die "option $1 requires a value"
}

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
at_time=""
workdir="$project_root"
model=""
sandbox="workspace-write"
timeout_seconds=3600
log_file=""
prompt_file=""
prompt=""
dry_run=false
codex_extra_args=()

while (($# > 0)); do
  case "$1" in
    -a|--at)
      need_value "$@"
      at_time="$2"
      shift 2
      ;;
    -C|--workdir)
      need_value "$@"
      workdir="$2"
      shift 2
      ;;
    -m|--model)
      need_value "$@"
      model="$2"
      shift 2
      ;;
    -s|--sandbox)
      need_value "$@"
      sandbox="$2"
      shift 2
      ;;
    -t|--timeout)
      need_value "$@"
      timeout_seconds="$2"
      shift 2
      ;;
    -l|--log-file)
      need_value "$@"
      log_file="$2"
      shift 2
      ;;
    -f|--prompt-file)
      need_value "$@"
      prompt_file="$2"
      shift 2
      ;;
    --codex-arg)
      need_value "$@"
      codex_extra_args+=("$2")
      shift 2
      ;;
    --dry-run)
      dry_run=true
      shift
      ;;
    --)
      shift
      [[ $# -le 1 ]] || die "put the whole prompt in one quoted argument after --"
      [[ $# -eq 1 ]] && prompt="$1"
      shift
      break
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1 (use -- before the prompt)"
      ;;
  esac
done

[[ "$sandbox" == read-only || "$sandbox" == workspace-write || "$sandbox" == yolo ]] ||
  die "invalid sandbox mode: $sandbox"
if [[ "$sandbox" == yolo ]]; then
  codex_sandbox="danger-full-access"
else
  codex_sandbox="$sandbox"
fi
[[ "$sandbox" != danger-full-access ]] || sandbox="danger-full-access"
[[ "$timeout_seconds" =~ ^[0-9]+$ ]] || die "timeout must be a non-negative integer"
(( timeout_seconds > 0 )) || die "timeout must be greater than zero"
[[ -d "$workdir" ]] || die "working directory does not exist: $workdir"
workdir="$(cd -- "$workdir" && pwd)"

if [[ -n "$prompt_file" ]]; then
  [[ -z "$prompt" ]] || die "use either a prompt argument or --prompt-file, not both"
fi

[[ -n "$prompt" || -n "$prompt_file" ]] || die "no prompt provided"

if [[ "$prompt_file" == "-" ]]; then
  prompt_source="/dev/stdin"
else
  prompt_source="$prompt_file"
fi

if [[ -n "$prompt_file" && "$prompt_file" != "-" && ! -r "$prompt_file" ]]; then
  die "prompt file is not readable: $prompt_file"
fi

export PATH="$HOME/.local/bin:$PATH"
codex_bin="${CODEX_BIN:-}"
if [[ -z "$codex_bin" ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex_bin="$(command -v codex)"
  elif [[ -x "$HOME/.local/bin/codex" ]]; then
    codex_bin="$HOME/.local/bin/codex"
  else
    die "codex not found; install it or set CODEX_BIN"
  fi
fi

target_epoch=""
if [[ -n "$at_time" ]]; then
  target_epoch="$(date -d "$at_time" +%s)" || die "invalid target time: $at_time"
  now_epoch="$(date +%s)"
  if (( target_epoch <= now_epoch )) && [[ "$at_time" =~ ^[0-9]{1,2}:[0-9]{2}(:[0-9]{2})?$ ]]; then
    target_epoch="$(date -d "tomorrow $at_time" +%s)" || die "invalid target time: $at_time"
  fi
  if (( target_epoch < $(date +%s) )); then
    die "target time is already in the past: $at_time"
  fi
fi

run_stamp="$(date +%Y%m%d-%H%M%S)"
if [[ -z "$log_file" ]]; then
  log_dir="${CODEX_TASK_LOG_DIR:-$HOME/.codex/scheduled-task-logs}"
  log_file="$log_dir/$run_stamp.log"
else
  :
fi
last_message_file="${log_file%.log}.last-message.txt"

cmd=(
  timeout "--kill-after=30s" "${timeout_seconds}s" "$codex_bin" exec
  --sandbox "$codex_sandbox"
  --color never
  -C "$workdir"
  -o "$last_message_file"
)
[[ -z "$model" ]] || cmd+=(-m "$model")
cmd+=("${codex_extra_args[@]}" -)

if $dry_run; then
  if [[ -n "$target_epoch" ]]; then
    wait_seconds=$((target_epoch - $(date +%s)))
    printf 'Would wait %s seconds (until %s), then run:\n' \
      "$wait_seconds" "$(date -d "@$target_epoch" '+%F %T %Z')"
  else
    printf 'Would run now:\n'
  fi
  printf ' %q' "${cmd[@]}"
  printf '\n'
  if [[ -n "$prompt" ]]; then
    printf 'Prompt: %s\n' "$prompt"
  else
    printf 'Prompt file: %s\n' "$prompt_source"
  fi
  exit 0
fi

mkdir -p "$(dirname -- "$log_file")"

{
  printf 'timestamp: %s\n' "$(date '+%F %T %Z')"
  printf 'workdir: %s\n' "$workdir"
  printf 'sandbox: %s\n' "$sandbox"
  printf 'timeout: %ss\n' "$timeout_seconds"
  printf 'log: %s\n' "$log_file"
  printf 'final reply: %s\n' "$last_message_file"
} >>"$log_file"

if [[ -n "$target_epoch" ]]; then
  wait_seconds=$((target_epoch - $(date +%s)))
  if (( wait_seconds > 0 )); then
    printf 'scheduled_at: %s\n' "$(date -d "@$target_epoch" '+%F %T %Z')" >>"$log_file"
    sleep "$wait_seconds"
  fi
fi

set +e
if [[ -n "$prompt" ]]; then
  printf '%s\n' "$prompt" | "${cmd[@]}" >>"$log_file" 2>&1
else
  "${cmd[@]}" <"$prompt_source" >>"$log_file" 2>&1
fi
status=$?
set -e

printf 'exit_status: %s\n' "$status" >>"$log_file"
if (( status == 0 )); then
  printf 'Codex task finished. Log: %s\n' "$log_file"
else
  printf 'Codex task failed with status %s. Log: %s\n' "$status" "$log_file" >&2
fi
exit "$status"
