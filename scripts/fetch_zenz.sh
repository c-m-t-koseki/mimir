#!/bin/sh
# Fetch a zenz model (azooKey's kana-kanji GPT-2) into a directory karukan
# can load as a local `[models]` entry: model.gguf + tokenizer.json.
#
# zenz-v3 uses the same prompt layout as jinen (context, input, output), so
# only two things need fixing up:
#   * the GGUF names a pre-tokenizer (`gpt2-small-japanese-char`) that only
#     azooKey's llama.cpp fork knows; mainline llama.cpp refuses to load it.
#     karukan tokenizes with tokenizer.json, so the key is simply dropped.
#   * no tokenizer.json is published; it is built from the HF repo's
#     vocab.json/merges.txt (the v3.x models share one 6000-token vocab).
#
# usage: scripts/fetch_zenz.sh [variant] [dest]
#   variant: zenz-v3.2-small (default), zenz-v3.2-xsmall, zenz-v3.1-small, ...
#   dest:    ~/Library/Application Support/com.karukan.karukan-im/models/<variant>
set -eu

variant=${1:-zenz-v3.2-small}
dest=${2:-"$HOME/Library/Application Support/com.karukan.karukan-im/models/$variant"}
mkdir -p "$dest"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fL --progress-bar -o "$tmp/model.gguf" \
    "https://huggingface.co/Miwa-Keita/$variant-gguf/resolve/main/ggml-model-Q5_K_M.gguf"
uv run --quiet --with gguf gguf-new-metadata --force \
    --remove-metadata tokenizer.ggml.pre "$tmp/model.gguf" "$dest/model.gguf" >/dev/null
uv run --quiet --with transformers --with tokenizers python - "$dest/tokenizer.json" <<'EOF'
import sys
from transformers import AutoTokenizer

AutoTokenizer.from_pretrained("Miwa-Keita/zenz-v3.1-small").backend_tokenizer.save(sys.argv[1])
EOF
echo "Installed $variant to $dest"
