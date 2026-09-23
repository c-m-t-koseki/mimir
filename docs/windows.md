# Windows 版（作業中）

Windows 版は、TSF（Text Services Framework）のテキストサービスとして動く。macOS 版と同じく、変換はすべて別プロセスの `mimir-imserver`（stdio の JSON-RPC）が行い、Windows 側はキーを渡して結果を文書に書き込むだけの薄いフロントエンドにする。

```
アプリ ─ TSF ─ mimir_tsf.dll（テキストサービス、アプリのプロセス内）
                  │ JSON-RPC（stdin/stdout）
                  └ mimir-imserver.exe（エンジン・モデル・学習）
```

コードは `mimir-im/windows/`（crate `mimir-tsf`）にある。

| ファイル | 内容 | 確認 |
|---|---|---|
| `src/client.rs` | サーバの起動と JSON-RPC | どの OS でも `cargo test -p mimir-tsf`（実物のサーバと話す） |
| `src/keymap.rs` | 仮想キーコード → XKB keysym | どの OS でもテストできる |
| `src/tsf/mod.rs` | DLL の入口、クラスファクトリ、TSF への登録 | Windows のみ |
| `src/tsf/service.rs` | テキストサービス: キーの受け取り、composition（preedit）と確定の書き込み | Windows のみ |

TSF の部分は macOS 上で `cargo check -p mimir-tsf --target x86_64-pc-windows-msvc` を通しているが、Windows の実機ではまだ動かしていない。

## ビルドとインストール

必要なもの: Rust（MSVC ツールチェーン）、Visual Studio Build Tools（C++）、CMake（llama.cpp のビルドに使う）。

管理者の PowerShell で:

```powershell
.\mimir-im\windows\install.ps1
```

`C:\Program Files\Mimir` に `mimir_tsf.dll` と `mimir-imserver.exe` を置き、システム辞書とモデルを取得して、`regsvr32` で登録する。そのあと 設定 → 時刻と言語 → 言語と地域 → 日本語 → 言語のオプション → キーボード で Mimir を追加する。外すときは `install.ps1 -Uninstall`。

設定・辞書・学習は `%APPDATA%\mimir\mimir-im\`（設定は `config\config.toml`、データは `data\`）に置かれる。

## キー

macOS 版と同じ（[part-input.md](part-input.md)）。JIS キーボードでは、無変換と英数で英数モード、変換・かな・カタカナ/ひらがなでかなモードに切り替える。

## 今できること

- キーをエンジンに渡し、漢字パートのライブ変換を composition として表示し、確定を書き込む。
- Ctrl+N / Ctrl+P で候補を切り替える（候補は preedit に出る）。
- アプリが composition を打ち切ったとき（別の場所のクリックなど）は、表示のまま確定する。

## まだのこと

優先度の高い順。

1. **サーバを全アプリで一つにする**: TSF のテキストサービスはアプリごとのプロセスに読み込まれるので、今はアプリごとに `mimir-imserver` を起動し、モデルもアプリごとに読み込む。名前付きパイプで一つのサーバにつなぐ形にする。サンドボックス化されたアプリ（ストアアプリ）はプロセスを起動できないので、この形が前提になる。
2. **候補窓**: 候補一覧と選択キー（asdfjkl）の表示。今は preedit で候補を切り替えるだけ。
3. **composition の下線**（表示属性、`ITfDisplayAttributeProvider`）。今は下線が付かないアプリがある。
4. **周辺テキスト**: カーソル左の文字列を文脈としてエンジンに渡す（`ITfRange::GetText`）。
5. **モード表示**: 言語バー（`ITfLangBarItemButton`）のあ / A。
6. **32 ビットアプリ**: 32 ビットのアプリには 32 ビット版の DLL（`i686-pc-windows-msvc`）も登録する。
7. **設定ウィンドウ**。
