# Dictionary

mimir はモデル推論に加えて、システム辞書・ユーザー辞書からの変換候補を提供します。辞書の構築・管理ツールについては [mimir-cli の README](../mimir-cli/README.md) を参照してください。

> [!NOTE]
> モデル推論だけでは語彙が限られるため、システム辞書の併用を強く推奨します。システム辞書はIMEに同梱されていないため、別途インストールが必要です。

## System Dictionary

double-array trieベースのシステム辞書です。

- デフォルトパス: `~/.local/share/mimir-im/dict.bin`（macOS: `~/Library/Application Support/com.mimir.mimir-im/dict.bin`）
- `dict_path` で任意のパスを指定可能（[Configuration](configuration.md) 参照）
- ファイルが存在しない場合は辞書なしで動作

ビルド済みの辞書を以下からダウンロードして配置できます:

```bash
# Linux
wget https://github.com/togatoga/karukan/releases/latest/download/dict.tgz
tar xzf dict.tgz
mkdir -p ~/.local/share/mimir-im
cp dict.bin ~/.local/share/mimir-im/

# macOS
curl -LO https://github.com/togatoga/karukan/releases/latest/download/dict.tgz
tar xzf dict.tgz
mkdir -p ~/Library/"Application Support"/com.mimir.mimir-im
cp dict.bin ~/Library/"Application Support"/com.mimir.mimir-im/
```

自分でビルドする場合は [mimir-cli の README](../mimir-cli/README.md) を参照してください。

### Mozc のコストで付け直す

配布している `dict.bin` のコストは Sudachi の値で、形態素解析のためのものです。IME の候補順には合わないものがあり、たとえば読み「かく」の 描く は 99999 で、まず出ません。`scripts/mozc_costs.py` は、IME 用に作られた Mozc の辞書（`dictionary_oss`）のコストで付け直し、動詞の過去形・て形（かいた → 書いた / 描いた、よんだ → 読んだ）を見出しに足します。

```bash
git clone --depth 1 https://github.com/google/mozc
cd mimir-cli
cargo run --release --bin mimir-dict -- view -a dict.bin > dict.tsv
../scripts/mozc_costs.py dict.tsv ../mozc/src/data/dictionary_oss > dict.json
cargo run --release --bin mimir-dict -- build dict.json -o dict.bin
```

- 両方の辞書にある（読み, 表記）は、Mozc のコストを今の尺度に直して使う（両方の10%点と90%点を合わせる一次式。Mozc の順位はそのまま残る）。
- 動詞の語幹（五段の連用タ接続、一段の連用形）に た・て（音便によっては だ・で）を付けた見出しを足す。
- Mozc にしかないそのほかの見出しは足さない。

Mozc の辞書は IPAdic・沖縄辞書と Google の追加分からなる。できた `dict.bin` を配るときは [THIRD_PARTY_LICENSES](../THIRD_PARTY_LICENSES) の表記を添える。

## User Dictionary

ユーザー辞書ディレクトリにファイルを配置すると、ユーザー辞書として読み込まれます。対応形式と登録方法の詳細は [user-dictionary.md](user-dictionary.md) を参照してください。

- デフォルトパス: `~/.local/share/mimir-im/user_dicts/`（macOS: `~/Library/Application Support/com.mimir.mimir-im/user_dicts/`）
- ディレクトリ内のファイルはすべて自動で読み込み（KRKNバイナリ・Mozc TSV を自動判定）
- ディレクトリが存在しない場合はユーザー辞書なしで動作

## 変換候補の優先順位

1. 📝 学習キャッシュ
2. 👤 ユーザー辞書
3. 🤖 モデル推論
4. 📚 システム辞書（スコア順）
5. ひらがな / カタカナ
6. 🔄 Rewriter（半角カタカナ・英字全角半角・記号バリアント）
