<div align="center">
  <img src="icon.png" width="128" alt="mimir" />
  <h1>Mimir</h1>
  <p>Linux・macOS・Windows（作業中）向け日本語入力システム — ニューラルかな漢字変換エンジン</p>
  <p><a href="https://github.com/togatoga/karukan">togatoga/karukan</a> の fork</p>

  [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
</div>

<div align="center">
  <img src="images/demo.gif" width="800" alt="mimir demo" />
</div>

## プロジェクト構成

IME本体(コアエンジン + 各プラットフォームのフロントエンド)は `mimir-im/` 配下にまとまっています。

- [mimir-im/](mimir-im/) — IME本体
  - [core/](mimir-im/core/) — 共有IMEエンジン(crate: `mimir-im`) — ステートマシン、ローマ字変換、mimir-imserver(macOS・Windows向けJSON-RPCサーバー)
  - [fcitx5/](mimir-im/fcitx5/) — Linux向けフロントエンド(crate: `mimir-fcitx5`) — fcitx5アドオン + C FFI
  - [macos/](mimir-im/macos/) — macOS向けフロントエンド — Swift/InputMethodKit
  - [windows/](mimir-im/windows/) — Windows向けフロントエンド（作業中） — TSFテキストサービス + 設定ウィンドウ（Tauri）
- [mimir-engine/](mimir-engine/) — コアライブラリ — ローマ字→ひらがな変換 + llama.cppによるニューラルかな漢字変換
- [mimir-cli/](mimir-cli/) — CLIツール・サーバー — 辞書ビルド、Sudachi辞書生成、辞書ビューア、AJIMEE-Bench、HTTPサーバー

## 特徴

- **ニューラルかな漢字変換**: GPT-2/Qwen3ベースのモデル（既定は jinen-v2）をllama.cppで推論し、高度な日本語変換。使うモデルは `config.toml` の `[models]` で差し替えられる（[比較](docs/model-bench.md)）
- **ライブ変換**: 入力と同時に変換結果をリアルタイム表示。Spaceを押さずに変換が進む（`Ctrl+Shift+L` でON/OFF）
- **コンテキスト対応**: 周辺テキストを考慮した日本語変換
- **変換学習**: ユーザーが選択した変換結果を記憶し、次回以降の変換で優先表示。予測変換（前方一致）にも対応し、入力途中でも学習済みの候補を提示
- **システム辞書**: [SudachiDict](https://github.com/WorksApplications/SudachiDict)の辞書データからシステム辞書を構築
- **候補リライター (Mozcから移植)**: 半角カタカナ、英字の大文字小文字・全角半角、記号の関連候補、数字の各種表記（漢数字・大字・ローマ数字・丸数字・16/8/2進数）を自動生成。各候補にはMozc由来の注釈（「半角カタカナ」「16進数」など）が付く
- **絵文字入力**: かな読み（`ぴえん` → 🥺、`きんにく` → 💪）と Slack 風 `:trigger` クエリ（`:smile` → 😄、`:halo` → 😇）の両方をサポート
- **漢字パート入力（macOS・Windows）**: 変換する範囲（パート）ごとにライブ変換し、Space で区切って確定する。SKK 式（大文字でパートを開く）も選べる。過去に確定した語の補完（Tab）、単語登録（Ctrl+R）、Google 日本語入力と同じ `z` 入力（`z-` → 〜 など）に対応
- **日付・時刻の変換**: `きょう` `あした` `いま` などを、その日付・時刻に変換する。形式は `config.toml` で変えられる
- **英単語のローマ字入力**: かなモードのまま英単語を打つと、辞書の英字表記に戻して変換する（`applenoshinseihin` → Appleの新製品、`yahoochiebukuro` → Yahoo知恵袋）。辞書への登録は要らない

<div align="center">
  <img src="images/live-conversion.gif" width="684" alt="文脈によるライブ変換と英単語の入力" />
  <p>打つそばから文脈で変換する。後ろを打つと前も直る（橋で → 箸で食べる、申請日 → 新製品）</p>
</div>

> **Note:** 初回起動時にHugging Faceからモデルをバックグラウンドでダウンロードします。ダウンロード中もかな入力と辞書変換はそのまま使え、モデルの読み込みが完了すると自動でニューラル変換が有効になります。ネットワークを使うのはこの初回ダウンロードだけで、2回目以降はダウンロード済みのモデルが使われます。

## インストール

- **Linux (fcitx5)**: [mimir-fcitx5 の README](mimir-im/fcitx5/README.md#install) を参照
- **macOS**: [mimir-macos の README](mimir-im/macos/README.md) を参照
- **Windows（作業中、実機では未確認）**: [Windows 版](docs/windows.md) を参照

## ドキュメント

- [キーバインド一覧](docs/key-bindings.md) — 共通キーバインドと Linux / macOS 固有キー
- [漢字パート入力](docs/part-input.md) — macOS・Windows のパート入力、SKK 式、候補窓、補完、英単語の入力
- [設定](docs/configuration.md) — config.toml の設定項目、ライブ変換、変換ストラテジー、学習キャッシュ
- [辞書](docs/dictionary.md) — システム辞書のインストール、ユーザー辞書、候補の優先順位
- [ユーザー辞書](docs/user-dictionary.md) — 対応形式（Mozc/Google IME TSV・バイナリ）と登録方法
- [Chunk](docs/chunking.md) — 変換が Chunk に区切られる場所と、自分で区切って表示を固定する方法
- [記号・半角全角](docs/symbols.md) — 句読点や括弧の種類、記号・数字・英字の幅、スペースの設定
- [日付・時刻の変換](docs/date.md) — `きょう` などのフレーズと出力形式
- [変換モデルの比較](docs/model-bench.md) — AJIMEE-Bench での精度と速度
- [Windows 版](docs/windows.md) — 構成、ビルドとインストール、今できること

## upstream との関係

Mimir は、[togatoga/karukan](https://github.com/togatoga/karukan)（Karukan、作者 togatoga）を fork したものです。ニューラルかな漢字変換のエンジン、ライブ変換と Chunk、変換学習、システム辞書、Mozc から移植した候補リライター、絵文字入力、fcitx5 版と macOS 版の土台、CLI ツールは karukan のものです。

fork したのは karukan の `3828dd2`（2026-09-19、「モデル定義をconfig.tomlに一本化してmodels.tomlを廃止する」）の時点です。そこから、この fork で主に次のことを変えています。

- **名前:** crate・バイナリ・設定とデータの置き場・macOS のバンドル ID を、karukan から Mimir に変えた（設定は `~/Library/Application Support/com.mimir.mimir-im/` など）。アイコンも替えた
- **macOS の入力:** 漢字パート入力と SKK 式、選択キー（`j k f d u i r e`）の候補窓、IME の中での英数・かなの切り替え（入力モードを2つ登録）、`z` 入力、単語登録、補完、設定ウィンドウ
- **学習と補完:** 学習するのはモデルの答えと違う語を選んだときだけにし、補完は別の入力の履歴（`history.tsv`）から出す。複数のエンジンで学習と履歴を共有する
- **英単語のローマ字入力:** かなモードのまま打った英単語を英字にする
- **Windows 版（作業中）:** TSF のテキストサービスと設定ウィンドウを足した
- **モデル:** zenz を読めるようにし、[比較](docs/model-bench.md) を足した

fork した後に karukan に入った変換ペルソナ（`docs/persona.md`）は、まだ取り込んでいません。先頭の `images/demo.gif` は karukan の README にあるデモをそのまま使っています。

## ライセンス

MIT OR Apache-2.0 のデュアルライセンスで提供しています。

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

[mimir-engine/data/](mimir-engine/data/) 配下には [Mozc](https://github.com/google/mozc) から派生したデータを含み、こちらは [BSD 3-Clause License](http://opensource.org/licenses/BSD-3-Clause) のもとで配布されています。各派生ファイルの由来およびMozcの著作権表記は [THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES) を参照してください。
