#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
VENV_DIR="$SCRIPT_DIR/.venv"
DATA_DIR="$SCRIPT_DIR/data"
DATASET_FILE="$DATA_DIR/longmemeval_s_cleaned.json"

if [ -f "$DATASET_FILE" ]; then
    echo "Dataset already exists at $DATASET_FILE"
    exit 0
fi

# Always recreate venv to avoid stale interpreter paths
if [ -d "$VENV_DIR" ]; then
    rm -rf "$VENV_DIR"
fi
echo "Creating venv at $VENV_DIR..."
python3 -m venv "$VENV_DIR"

echo "Installing huggingface_hub..."
"$VENV_DIR/bin/pip" install -q huggingface_hub

echo "Downloading LongMemEval-S dataset (264 MB)..."
mkdir -p "$DATA_DIR"
"$VENV_DIR/bin/python3" -c "
from huggingface_hub import hf_hub_download
hf_hub_download(
    repo_id='xiaowu0162/longmemeval-cleaned',
    filename='longmemeval_s_cleaned.json',
    repo_type='dataset',
    local_dir='$DATA_DIR',
)
"

echo "Done. Dataset at $DATASET_FILE"
