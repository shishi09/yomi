#!/bin/bash
set -e
cd "$(dirname "$0")"

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Démarre Ollama automatiquement si installé et pas encore lancé
if command -v ollama >/dev/null 2>&1; then
  if ! curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
    echo "[Ollama] Démarrage du moteur local..."
    brew services start ollama >/dev/null 2>&1 || (OLLAMA_FLASH_ATTENTION="1" OLLAMA_KV_CACHE_TYPE="q8_0" nohup ollama serve >/tmp/ollama-manga.log 2>&1 &)
    sleep 2
  fi
fi

exec python3 server.py
