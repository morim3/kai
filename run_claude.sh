#!/bin/bash

while true; do
    COMMIT=$(git rev-parse --short=6 HEAD)
    LOGFILE="agent_logs/agent_${COMMIT}.log"

    claude --dangerously-skip-permissions \
           -p "$(cat CLAUDE.md)" \
           --model claude-opus-4-6 2>&1 | tee "$LOGFILE"
done
