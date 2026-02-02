#!/bin/bash
# Stop hook: Check if Claude made excuses about pre-existing bugs
# If found, block stopping and force continuation

# Read JSON input
INPUT=$(cat)
TRANSCRIPT=$(echo "$INPUT" | jq -r '.transcript_path')
STOP_HOOK_ACTIVE=$(echo "$INPUT" | jq -r '.stop_hook_active')

# Prevent infinite loop - if we're already continuing from a stop hook, allow stop
if [ "$STOP_HOOK_ACTIVE" = "true" ]; then
  exit 0
fi

# Get last assistant message text from transcript
if [ -f "$TRANSCRIPT" ]; then
  # Get last assistant message line and extract text content
  LAST_MSG=$(grep '"type":"assistant"' "$TRANSCRIPT" | tail -1 | jq -r '.message.content[]? | select(.type=="text") | .text' 2>/dev/null)

  # Check for excuse patterns (case insensitive)
  if echo "$LAST_MSG" | grep -qiE 'pre-existing|already.?(failing|broken|failed)|not.?related.?to.?(my|the|current)|was.?(broken|failing).?before|existing.?issue|this.?was.?already|unrelated.?to'; then
    # Block stopping, force continuation with reminder
    cat <<'EOF'
{
  "decision": "block",
  "reason": "You just dismissed a bug as pre-existing or unrelated. Per project guidelines: ALL bugs must be fixed regardless of whether they are pre-existing. If you found it, fix it. Go back and fix the issue you just mentioned."
}
EOF
    exit 0
  fi
fi

# No excuses found, allow stop
exit 0
