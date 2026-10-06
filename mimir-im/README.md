# mimir-im

Mimir の IME 本体。共有エンジンと各プラットフォームのフロントエンドがここにまとまっています。

| ディレクトリ | 説明 |
|---------|------|
| [core/](core/) | 共有IMEエンジン(crate: `mimir-im`) — ステートマシン、ローマ字変換、mimir-imserver(macOS・Windows向けJSON-RPCサーバー) |
| [fcitx5/](fcitx5/) | Linux向けフロントエンド(crate: `mimir-fcitx5`) — fcitx5アドオン + C FFI |
| [macos/](macos/) | macOS向けフロントエンド — Swift/InputMethodKit |
| [windows/](windows/) | Windows向けフロントエンド（作業中） — TSFテキストサービス(crate: `mimir-tsf`) + 設定ウィンドウ(crate: `mimir-settings`、Tauri)。[説明](../docs/windows.md) |

キーバインド・設定・辞書などユーザー向けドキュメントは [docs/](../docs/) を、インストール手順は各フロントエンドの README（Windows は [docs/windows.md](../docs/windows.md)）を参照してください。
