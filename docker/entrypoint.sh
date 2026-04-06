#!/bin/bash
set -e

echo "Starting Anki (headless, QT_QPA_PLATFORM=offscreen)..."
anki &
ANKI_PID=$!

echo "Waiting for AnkiConnect on port 8765..."
for i in $(seq 1 60); do
    if curl -s http://localhost:8765 -X POST \
        -d '{"action":"version","version":6}' > /dev/null 2>&1; then
        echo "AnkiConnect is ready!"
        break
    fi
    sleep 1
done

# Verify it's actually working
VERSION=$(curl -s http://localhost:8765 -X POST \
    -d '{"action":"version","version":6}' 2>/dev/null || echo "FAILED")
echo "AnkiConnect response: $VERSION"

# Keep container alive
wait $ANKI_PID
