#!/bin/bash
# Stop hook: remind Claude that "it was already broken" is not an excuse

input=$(cat)
transcript_path=$(echo "$input" | jq -r '.transcript_path // empty')

if [[ -z "$transcript_path" || ! -f "$transcript_path" ]]; then
  echo '{"decision": "approve"}'
  exit 0
fi

# Get all assistant text from the current turn (after the last user message)
current_turn_text=$(
  jq -s '
    [to_entries[] | {idx: .key, type: .value.type, texts: [.value.message.content[]? | select(.type == "text") | .text]}] |
    ([.[] | select(.type == "user") | .idx] | max // -1) as $last_user |
    [.[] | select(.type == "assistant" and .idx > $last_user) | .texts[]] |
    join("\n")
  ' "$transcript_path" 2>/dev/null
)

# Check for excuse patterns
if echo "$current_turn_text" | grep -qiE 'pre-existing|preexisting|already existed|existed before|was already broken|already failing'; then
  echo '{"decision": "block", "reason": "🚨 You mentioned something being pre-existing. This is NOT an excuse. Fix ALL failures regardless of when they were introduced."}'
else
  echo '{"decision": "approve"}'
fi
