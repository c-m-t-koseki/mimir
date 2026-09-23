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
