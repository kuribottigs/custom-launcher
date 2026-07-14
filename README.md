# Oxide Launcher

Rust 製の Minecraft カスタムランチャーです。[Modrinth App](https://modrinth.com/app) や Prism Launcher のように、複数インスタンスの管理・MOD の検索/導入・Microsoft アカウントでのログイン・実際のゲーム起動までを 1 つのアプリで行えます。GUI は Zed エディタの UI フレームワーク [gpui](https://www.gpui.rs/) で実装しています。

## 主な機能

- **Microsoft アカウントログイン** — デバイスコードフロー（MSA → Xbox Live → XSTS → Minecraft Services）。リフレッシュトークンによる自動セッション更新付き
- **オフラインアカウント** — バニラ互換のオフライン UUID を生成（`OfflinePlayer:<名前>` の MD5）
- **バージョン管理** — Mojang の version manifest からリリース/スナップショットを取得し、クライアント本体・ライブラリ・アセットを SHA-1 検証付きで並列ダウンロード
- **Fabric 対応** — Fabric meta API からローダープロファイルを取得してバニラとマージ
- **Modrinth 連携** — MOD の検索と、インスタンスの `mods/` フォルダへのワンクリック導入
- **インスタンス管理** — バージョン/ローダー/メモリ設定ごとに独立したゲームディレクトリ
- **Java 自動検出** — 要求メジャーバージョン（例: 1.21 → Java 21）に合う JVM を探索
- **GUI + CLI** — gpui 製のデスクトップ GUI と、同じコアを使う `oxide` CLI

## 構成

```
crates/
  launcher-core/   コアライブラリ（認証・ダウンロード・起動・Modrinth・インスタンス）
  launcher-cli/    CLI (バイナリ名: oxide)
  launcher-gui/    gpui 製 GUI (バイナリ名: oxide-launcher)
```

## ビルド

Rust 1.85 以降（edition 2024）が必要です。

### Linux の依存パッケージ

gpui のビルドに以下が必要です（Ubuntu/Debian の例）:

```sh
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
    libx11-xcb-dev libxcb1-dev libvulkan-dev libfontconfig-dev pkg-config
```

macOS では Xcode Command Line Tools のみで、追加パッケージは不要です。

### ビルドと起動

```sh
# GUI
cargo run --release -p launcher-gui

# CLI
cargo run --release -p launcher-cli -- --help
```

## Microsoft ログインの設定

Minecraft の認証 API を利用するには、自分の Azure アプリケーション（クライアント ID）が必要です。

1. [Azure Portal](https://portal.azure.com/) → App registrations → New registration
   - Supported account types: **Personal Microsoft accounts** を含める
   - Authentication で **Allow public client flows** を有効化（デバイスコードフローに必要）
2. Mojang に [Minecraft API 利用申請](https://help.minecraft.net/hc/en-us/articles/16254801392141) を提出し、クライアント ID を承認してもらう
3. 取得したクライアント ID を設定:
   - GUI: 「設定」タブ →「Microsoft ログイン用クライアントID」
   - CLI/共通: 環境変数 `MSA_CLIENT_ID`、または `settings.json` の `msa_client_id`

> クライアント ID 未設定でも**オフラインアカウント**での起動は可能です（正規のアカウント所持者向けの検証用途を想定）。

## 使い方（CLI）

```sh
oxide login                 # Microsoftログイン（デバイスコード）
oxide login-offline Steve   # オフラインアカウント追加
oxide accounts              # アカウント一覧

oxide versions              # バージョン一覧
oxide create mypack --version 1.21.4 --fabric
oxide list

oxide search sodium --version 1.21.4 --loader fabric
oxide install sodium mypack # ModrinthのMODを導入

oxide launch mypack             # ダウンロード→起動
oxide launch mypack --dry-run   # 起動コマンドの確認のみ
```

## データディレクトリ

既定では OS のデータディレクトリ（Linux: `~/.local/share/oxide-launcher`）に保存します。環境変数 `OXIDE_LAUNCHER_HOME` で変更できます。

```
oxide-launcher/
  settings.json    ランチャー設定
  accounts.json    アカウント（トークンは平文保存。ディレクトリの権限に注意）
  meta/            バージョンマニフェスト等のキャッシュ
  versions/        クライアント jar
  libraries/       共有ライブラリ (maven レイアウト)
  assets/          共有アセット
  instances/<名前>/.minecraft/   各インスタンスのゲームデータ
```

## テスト

```sh
cargo test -p launcher-core
```

ユニットテストに加え、ローカル HTTP サーバーに対して「クライアント/ライブラリ/ネイティブ/アセットのダウンロード → 起動コマンド生成」までの統合テストが走ります（外部ネットワーク不要）。

## 既知の制限

- MOD ローダーは現在 **Fabric のみ**（Forge / NeoForge / Quilt は未対応）
- Modrinth の **modpack**（.mrpack）導入は未対応（MOD 単体の導入のみ）
- 必要な Java が見つからない場合の **JRE 自動ダウンロードは未対応**（システムの Java を使用）
- アカウントトークンは平文の JSON に保存されます
- GUI のスキン表示・アイコン画像表示は未実装です

## ライセンス

MIT
