#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SAMPLE_PROJECT="$REPO_ROOT/sample-project"
E2E_TIMEOUT="${E2E_TIMEOUT:-120}"
OUTPUT_FILE="$(mktemp)"
trap 'rm -f "$OUTPUT_FILE"' EXIT

cd "$SAMPLE_PROJECT"

if ! command -v opencode &>/dev/null; then
  echo "error: opencode not found in PATH"
  exit 1
fi

if ! command -v jq &>/dev/null; then
  echo "error: jq required for JSONL validation"
  exit 1
fi

echo "Running opencode (timeout ${E2E_TIMEOUT}s)..."
if ! timeout "$E2E_TIMEOUT" opencode run "Use the greet tool with the name sunshine" --format json >"$OUTPUT_FILE" 2>&1; then
  echo "opencode run failed or timed out"
  exit 1
fi

found=0
while IFS= read -r line; do
  if echo "$line" | jq -e '
    .type == "tool_use" and
    ((.part.tool // .part.name // .part.state.tool // "") | test("greet")) and
    ((. | tostring) | contains("Hello, sunshine"))
  ' >/dev/null 2>&1; then
    found=1
    break
  fi
done <"$OUTPUT_FILE"

if [[ "$found" -eq 1 ]]; then
  echo "PASS: greet tool was called and returned 'Hello, sunshine'"
  exit 0
else
  echo "FAIL: no tool_use event for greet with 'Hello, sunshine' in output"
  echo ""
  echo "Debug:"
  line_count=$(grep -c . "$OUTPUT_FILE" 2>/dev/null || echo 0)
  echo "  Output: ${line_count} lines"
  event_types=$(while IFS= read -r line; do echo "$line" | jq -r '.type // "null"'; done <"$OUTPUT_FILE" | sort -u)
  echo "  Event types: ${event_types:-none}"
  echo "  Error events:"
  error_count=0
  while IFS= read -r line; do
    type=$(echo "$line" | jq -r '.type // empty')
    if [[ "$type" == "error" ]]; then
      error_count=$((error_count + 1))
      name=$(echo "$line" | jq -r '.error.name // "?"')
      msg=$(echo "$line" | jq -r '.error.data.message // .error.message // .error.data // .error | tostring')
      echo "    [$error_count] $name: $msg"
    fi
  done <"$OUTPUT_FILE"
  [[ "$error_count" -eq 0 ]] && echo "    (none)"
  echo "  tool_use events:"
  tool_use_count=0
  while IFS= read -r line; do
    type=$(echo "$line" | jq -r '.type // empty')
    if [[ "$type" == "tool_use" ]]; then
      tool_use_count=$((tool_use_count + 1))
      tool=$(echo "$line" | jq -r '.part.tool // .part.name // .part.state.tool // "?"')
      output_snippet=$(echo "$line" | jq -r '(.part.state.output // .part.output // "") | tostring | if length > 120 then .[0:120] + "..." else . end')
      echo "    [$tool_use_count] tool=$tool output=${output_snippet}"
    fi
  done <"$OUTPUT_FILE"
  [[ "$tool_use_count" -eq 0 ]] && echo "    (none)"
  cp "$OUTPUT_FILE" "$SAMPLE_PROJECT/.e2e-opencode-last-run.jsonl"
  echo ""
  echo "Full output saved to sample-project/.e2e-opencode-last-run.jsonl"
  exit 1
fi
