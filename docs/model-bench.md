# 変換モデルの比較

AJIMEE-Bench（JWTD_v2、200 問、左文脈あり）を `ajimee-bench` で測った結果。
環境は Apple M1 / 16GB、量子化はすべて Q5_K_M。時間は 200 問の合計（モデル読込後、2 回測った値）で、1 問は 1 文程度の長さである。

| モデル | 完全一致 | 完全一致（NFKC） | 平均 CER（NFKC） | 時間 |
|---|---|---|---|---|
| jinen-v2-small | 79.0% | 85.0% | **0.0158** | 8.3 s |
| zenz-v3.2-small | 79.0% | **85.5%** | 0.0181 | 14.0 s |
| zenz-v3.1-small | 78.5% | 84.5% | 0.0214 | 14.0 s |
| jinen-v2-xsmall | 73.5% | 79.0% | 0.0273 | **3.4 s** |
| zenz-v3.2-xsmall | 68.0% | 73.0% | 0.0400 | 4.4 s |

small 級の2つは完全一致率が同じで、平均 CER は jinen-v2-small の方が低い。速さも jinen-v2-small が zenz-v3.2-small の約 1.7 倍ある。そこで、既定は upstream と同じく jinen-v2-small（メイン）と jinen-v2-xsmall（軽量）のままとする。zenz-v3.2-small は、右文脈とプロフィールを入力に取れるので、それらを使う段階で改めて比較する。

zenz は `scripts/fetch_zenz.sh` でローカルモデルとして取り込む（`[models]` にパスを書き、`[conversion] model` にそのキーを指定する。キーに `.` は使えない）。再計測の手順は次のとおり。

```sh
curl -fLO https://raw.githubusercontent.com/azooKey/AJIMEE-Bench/main/JWTD_v2/v1/evaluation_items.json
cargo run --release --bin ajimee-bench -- evaluation_items.json --model jinen-v2-small-q5 --quiet
cargo run --release --bin ajimee-bench -- evaluation_items.json \
    --gguf <dir>/model.gguf --tokenizer-json <dir>/tokenizer.json --quiet
```
