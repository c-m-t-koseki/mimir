# Windows 版（作業中）

Windows 版は、TSF（Text Services Framework）のテキストサービスとして動く。macOS 版と同じく、変換はすべて別プロセスの `mimir-imserver`（stdio の JSON-RPC）が行い、Windows 側はキーを渡して結果を文書に書き込むだけの薄いフロントエンドにする。

```
アプリ ─ TSF ─ mimir_tsf.dll（テキストサービス、アプリのプロセス内）
                  │ JSON-RPC（stdin/stdout）
                  └ mimir-imserver.exe（エンジン・モデル・学習）

入力インジケーターの「設定…」─ mimir-settings.exe（設定ウィンドウ、Tauri）
                                  │ JSON-RPC（stdin/stdout）
                                  └ mimir-imserver.exe（モデルを読まない）
```

コードは `mimir-im/windows/`（crate `mimir-tsf`）と `mimir-im/windows/settings/`（crate `mimir-settings`）にある。

| ファイル | 内容 | 確認 |
|---|---|---|
| `src/client.rs` | サーバの起動と JSON-RPC | どの OS でも `cargo test -p mimir-tsf`（実物のサーバと話す） |
| `src/keymap.rs` | 仮想キーコード → XKB keysym | どの OS でもテストできる |
| `src/tsf/mod.rs` | DLL の入口、クラスファクトリ、TSF への登録 | Windows のみ |
| `src/tsf/service.rs` | テキストサービス: キーの受け取り、composition（preedit）と確定の書き込み、フォーカスでの設定の追従 | Windows のみ |
| `src/tsf/langbar.rs` | 入力インジケーターのあ / A と、右クリックのメニュー | Windows のみ |
| `settings/src/app.rs` | 設定ウィンドウの Tauri 側: エンジンへの RPC、辞書の編集、モデルの計測 | macOS でも窓を開ける。`cargo test -p mimir-settings` |
| `settings/src/dict.rs` | `user_dicts/` の TSV の編集 | どの OS でもテストできる |
| `settings/ui/` | 設定ウィンドウの画面（HTML と JS） | macOS で窓を開いて確かめる |

TSF の部分と設定ウィンドウは macOS 上で `cargo check -p mimir-tsf -p mimir-settings --target x86_64-pc-windows-msvc` を通しているが、Windows の実機ではまだ動かしていない。設定ウィンドウは macOS でも `cargo run -p mimir-settings` で開いて確かめられる（macOS の IME が使うのは SwiftUI の設定のまま）。Linux ではスタブになり、WebKitGTK は要らない。

## ビルドとインストール

管理者の PowerShell で:

```powershell
.\mimir-im\windows\install.ps1
```

最初に、ビルドに要るものがそろっているかを確かめ、足りないものを winget で入れる（入れる前に一度だけ確認する）。

| 必要なもの | 用途 | 確かめ方 | winget の ID |
|---|---|---|---|
| Rust（MSVC ツールチェーン） | ビルド全体 | `cargo` | `Rustlang.Rustup` |
| Visual Studio Build Tools（C++） | リンカ、llama.cpp のコンパイル | `vswhere` で VC ツール | `Microsoft.VisualStudio.2022.BuildTools`（C++ ワークロード） |
| CMake | llama.cpp のビルド | `cmake` | `Kitware.CMake` |
| LLVM（libclang） | llama.cpp の bindgen | `libclang.dll` | `LLVM.LLVM` |
| WebView2 ランタイム | 設定ウィンドウの描画（Windows 11 には入っている） | レジストリの EdgeUpdate のキー | `Microsoft.EdgeWebView2Runtime` |

winget が無いときは、Microsoft Store の「アプリ インストーラー」を入れるか、上の4つを手で入れる。ビルドの準備だけしたいときは `install.ps1 -Setup`。

そろったら、ビルドして `C:\Program Files\Mimir` に `mimir_tsf.dll`・`mimir-imserver.exe`・`mimir-settings.exe` を置き（スタートメニューに「Mimir Settings」も作る）、システム辞書とモデルを取得して、`regsvr32` で登録する。そのあと 設定 → 時刻と言語 → 言語と地域 → 日本語 → 言語のオプション → キーボード で Mimir を追加する。外すときは `install.ps1 -Uninstall`。

設定・辞書・学習は `%APPDATA%\mimir\mimir-im\`（設定は `config\config.toml`、データは `data\`）に置かれる。

## キー

macOS 版と同じ（[part-input.md](part-input.md)）。JIS キーボードでは、無変換と英数で英数モード、変換・かな・カタカナ/ひらがなでかなモードに切り替える。

## 今できること

- キーをエンジンに渡し、漢字パートのライブ変換を composition として表示し、確定を書き込む。
- Ctrl+N / Ctrl+P で候補を切り替える（候補は preedit に出る）。
- アプリが composition を打ち切ったとき（別の場所のクリックなど）は、表示のまま確定する。
- タスクバーの入力インジケーターに、今のモード（あ / A）を出す。右クリックの「設定…」で設定ウィンドウが開く。

## 設定ウィンドウ

macOS の設定と同じ6タブ（一般・キー・モデル・辞書・学習・履歴）。自分用の `mimir-imserver` をモデルなしで起動し（`init` の `{"models": false}`）、設定は `get_config` / `set_config`、学習と履歴は `learning_*` / `history_*` で読み書きする。ユーザー辞書は `user_dicts/` の TSV を直接編集する。

アプリごとのエンジンは、設定を起動時にしか読まない。そこでテキストサービスは、アプリにフォーカスが戻るたびに config.toml と `user_dicts/` の更新時刻を起動時と比べ、変わっていればエンジンを起動し直す。変わっていなければ学習を保存し、そのとき他のエンジンが書いた分（設定ウィンドウでの削除など）を取り込む。学習と履歴の保存は、ファイルが前に読み書きしたときから変わっていれば、それを土台にして自分の変更（記録・削除・全消去）を重ねてから書く（`LearningCache::sync`）。これで、エンジンが複数あっても、ほかのエンジンの学習や設定ウィンドウでの削除を古い写しで上書きしない。

## まだのこと

優先度の高い順。

1. **サーバを全アプリで一つにする**: TSF のテキストサービスはアプリごとのプロセスに読み込まれるので、今はアプリごとに `mimir-imserver` を起動し、モデルもアプリごとに読み込む。名前付きパイプで一つのサーバにつなぐ形にする。サンドボックス化されたアプリ（ストアアプリ）はプロセスを起動できないので、この形が前提になる。
2. **候補窓**: 候補一覧と選択キー（j k f d u i r e）の表示。今は preedit で候補を切り替えるだけ。
3. **composition の下線**（表示属性、`ITfDisplayAttributeProvider`）。今は下線が付かないアプリがある。
4. **周辺テキスト**: カーソル左の文字列を文脈としてエンジンに渡す（`ITfRange::GetText`）。
5. **モード表示の左クリック**: あ / A は出るが、左クリックでモードを切り替えられない。
6. **32 ビットアプリ**: 32 ビットのアプリには 32 ビット版の DLL（`i686-pc-windows-msvc`）も登録する。
7. **設定ウィンドウを一つにする**: 「設定…」を二度選ぶと窓が二つ開く。
