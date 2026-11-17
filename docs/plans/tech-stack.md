# 技術選定: Windows と macOS のアプリのフレームワーク

## 目的・背景

vpn-on-demand は Windows と macOS のデスクトップアプリとして作る（2026-11-16 にユーザーが決めた）。1 つのコードベースから両方を作るためのフレームワークを決める。

アプリがすること（CLAUDE.md の「設計の方針」より）:

- 国を選んで接続・切断する画面と、キーを入力する設定画面
- タスクトレイ（Windows）/ メニューバー（macOS）に常駐し、そこから接続・切断する
- Vultr API v2 を HTTPS で呼ぶ（VPS の作成・一覧・削除、リージョンとプランの一覧）
- Tailscale の CLI（`tailscale status --json`、`tailscale set --exit-node=...`）を外部コマンドとして呼ぶ。または Tailscale の LocalAPI を使う
- キー（Vultr の API キー、Tailscale の auth key / OAuth クライアント）を OS の資格情報ストア（Windows の資格情報マネージャー、macOS のキーチェーン）に保存する
- 接続中に経過時間を数え、一定時間で自動で切断する（アプリが常駐している前提のタイマー）
- 配布: Windows のインストーラ、macOS の `.app` / `.dmg`。App Store では配らない（サンドボックスに入ると Tailscale の CLI を呼べないため）

開発の条件:

- コードは主に Claude Code が書く。Claude Code は sx360（Windows 11 + WSL2）の WSL と、m1（macOS）で動く
- Windows 版は sx360、macOS 版は m1 でビルドして確かめる

## 対応方針

### Step 1: 候補を机上で比べる

候補と、いまの見込み（Step 1 で公式のドキュメントで確かめ、出典と日付を付けて「調査結果」に書く）:

| 候補 | 言語 | 見込み |
|---|---|---|
| **Tauri 2** | Rust + Web（HTML/TS） | 第一候補。OS の WebView を使うので配布物が小さい（数 MB〜十数 MB）。トレイ、通知、自動更新、外部コマンドの呼び出し（shell プラグイン）が公式にある。資格情報ストアは `keyring` クレート。Windows は WebView2（Windows 11 には入っている） |
| Electron | TypeScript（Node.js） | 情報と実績がいちばん多い。配布物が大きい（100 MB 前後）。トレイは標準。資格情報は `safeStorage`（OS の鍵で暗号化してファイルに保存。keytar は開発終了） |
| Wails | Go + Web | Tauri に近い作り（OS の WebView）。Go で書ける。トレイの対応状況を確かめる（v3 で対応、v3 の安定度も確かめる） |
| Flutter | Dart | 画面を自前で描くので見た目が両 OS で揃う。トレイ、資格情報ストアはコミュニティのプラグイン（`tray_manager`、`flutter_secure_storage`）頼み |
| .NET（Avalonia） | C# | トレイは標準。資格情報ストアは OS ごとに書く必要がある。macOS の配布（署名、公証）の手間を確かめる |

比べる観点:

1. **トレイ / メニューバーの常駐**: 両 OS で、メニューから接続・切断でき、状態でアイコンを替えられるか。macOS でドックに出さない（メニューバーだけの）アプリにできるか
2. **OS の資格情報ストア**: 両 OS で、公式または保守されているライブラリで読み書きできるか
3. **外部コマンドと HTTP**: Tailscale の CLI を呼んで JSON を読めるか。Windows でコンソール画面を出さずに呼べるか。Vultr の API を呼べるか
4. **ビルド**: sx360 の WSL から Windows 版を作れるか（クロスコンパイル）、Windows 側でビルドが要るか。m1 で macOS 版（Apple Silicon、できれば Intel も）を作れるか
5. **配布**: インストーラの作りやすさ、配布物の大きさ、コード署名と公証の手順、自動更新
6. **保守の状況**: 開発が続いているか、2026-11 の時点の安定版
7. **書きやすさ**: Claude Code が書いて直しやすいか、型があるか、テストを書きやすいか

### Step 2: 試作で確かめる（上位 1〜2 候補）

Step 1 で絞った候補で、次だけを持つ最小のアプリを作り、sx360（Windows）と m1（macOS）で動かす。VPS は作らない（課金しない）。

- トレイ / メニューバーにアイコンを出し、メニューから「状態を見る」を選ぶと、Tailscale の CLI で `tailscale status --json` を読んで、自分のホスト名と出口ノードの有無を窓に出す
- 設定画面でダミーのキーを入力して資格情報ストアに保存し、アプリを起動し直して読み戻せる。資格情報マネージャー / キーチェーンアクセスで、保存されたことを目で確かめる
- Vultr の API の読み取りだけの呼び出し（`GET /v2/regions`）で、リージョンを国ごとにまとめて出す。キーの要らない API なので、`.env` のキーは使わない
- Windows のインストーラと macOS の `.app` を作り、署名なしで入れられるか（SmartScreen、Gatekeeper でどうなるか）を見る

試作は `poc/<候補>/` に置き、決めたあとに消す（決めた候補の試作は本体の出発点にしてよい）。

### Step 3: 決めて記録する

- 比較表と試作の結果をこのプランの「調査結果」に書き、ユーザーと相談して決める
- 決めたフレームワーク、言語、ディレクトリ構成、ビルドの手順を CLAUDE.md の「設計の方針」と「開発環境」に書く
- TODO.md の「開発環境を作る」以降のタスクを、決めたフレームワークに合わせて直す

## 影響範囲

- このリポジトリ: プラン、`poc/`（一時的）、CLAUDE.md、TODO.md
- sx360 と m1 に、試作のためのツールチェーンを入れる（例: Tauri なら Rust と Node.js、Windows には Visual Studio の Build Tools、macOS には Xcode の Command Line Tools）。**入れる前にユーザーに確認する**（CLAUDE.md の「注意が必要な操作」）。入れたものは「調査結果」に書き、選ばなかった候補のものは消すかをユーザーに聞く
- Tailscale の出口ノードは切り替えない（`status` を読むだけ）。VPS は作らない

## テスト方針

- Step 1 は調査なので、比較表の各項目に出典（公式のドキュメント）と確かめた日付を付ける
- Step 2 の試作は、上の 4 項目を sx360 と m1 の両方で実際に動かして確かめ、結果（できた / できない / 手間）を表にする。トレイと資格情報ストアは、ユーザーに画面で確かめてもらう

## 調査結果

### Step 1: 机上の比較（2026-11-16、公式のドキュメント・パッケージのレジストリ・Web 検索）

**各候補の最新版（2026-11-16 にレジストリで確かめた）**

| 候補 | 最新の安定版 | 関係する部品 |
|---|---|---|
| Tauri | `tauri` 2.12.6（2026-11-16） | `tauri-plugin-shell` 2.6.0（2026-10-28）、`tray-icon` 0.26.1（2026-10-07）、`keyring` 4.2.0（2026-08-29） |
| Electron | 45.3.3 | 8 週ごとに新しいメジャー版が出て、サポートは直近の 3 つだけ |
| Wails | v3.1.0（2026-11-01。v3 の最初の安定版の系列）、v2.17.1 | v3 の公式ドキュメントにはまだ「Beta」と書かれたページが残っている |
| Flutter | （SDK は未確認） | `tray_manager` 0.9.0（2026-11-12）、`flutter_secure_storage` 11.2.0（2026-09-16） |
| .NET（Avalonia） | Avalonia 12.1.6 | 資格情報ストアは別のライブラリ（例: `ktsu.CredentialCache`） |

**観点ごとの比較**

| 観点 | Tauri 2 | Electron | Wails v3 | Flutter | Avalonia |
|---|---|---|---|---|---|
| 1. トレイ / メニューバー | ◎ 公式（`TrayIconBuilder`）。macOS は `ActivationPolicy::Accessory` でドックに出さないメニューバーだけのアプリにできる（ドックのアイコンが残ることがあるという報告あり） | ◎ 標準の `Tray` | ○ v3 で対応（メニュー、ライト / ダークのアイコン、macOS のテンプレートアイコン）。安定 API の一覧にはまだ入っていない | △ コミュニティのプラグイン（`tray_manager`。2026-11 にも更新あり） | ○ 標準の `TrayIcon` |
| 2. 資格情報ストア | ◎ `keyring` クレート（Windows の資格情報マネージャー、macOS のキーチェーン）。v4 で `keyring-core` と OS ごとのストア（`windows-native-keyring-store`、`apple-native-keyring-store`）に分かれた | ○ `safeStorage`（Windows は DPAPI、macOS はキーチェーンの鍵で暗号化し、暗号文は自分でファイルに保存する）。macOS では署名しないと動きが安定しない。同期版の API は非推奨（Electron 46 で削除とする記述あり、要確認）。keytar は保守されていない | ○ Go の keychain ライブラリ（cgo なしのもの）がある | ○ `flutter_secure_storage` | △ OS ごとに書くか、小さなライブラリに頼る |
| 3. 外部コマンドと HTTP | ◎ Rust の標準の `std::process::Command`（Windows では `CREATE_NO_WINDOW` でコンソールを出さない）と HTTP クライアント（`reqwest` など）。画面からは呼ばず Rust 側で処理するので、shell プラグインは要らない | ◎ Node.js の `child_process`、`fetch` | ◎ Go の `os/exec`、`net/http` | ○ `dart:io` の `Process`、`http` | ◎ .NET の `Process`、`HttpClient` |
| 4. ビルド | ○ Windows 版は WSL から `cargo-xwin` と NSIS でクロスビルドできる（公式に記載。ただし「Windows で直接ビルドするほど簡単ではなく、あまりテストされていない」。MSI は Windows でしか作れない）。macOS 版は m1 でビルド | ○ Windows 版は Windows で、macOS 版は macOS でビルドするのが基本 | ○ Go はクロスコンパイルしやすいが、WebView の部分は OS ごとにビルドするのが基本 | △ デスクトップ版は OS ごとにその OS でビルド | ○ .NET は WSL から Windows 版を作りやすい。macOS の `.app` と署名は手間が多い |
| 5. 配布物の大きさ | ◎ 数 MB（hello world で約 3 MB。OS の WebView を使う。Windows 11 は WebView2 入り） | △ 85〜150 MB（Chromium を同梱） | ◎ 数 MB〜十数 MB | ○ 数十 MB | ○ 数十 MB（自己完結型の場合） |
| 5. 配布（署名・更新） | ○ 公式の updater プラグイン、Windows のコード署名、macOS の署名と公証の手順が公式ドキュメントにある | ◎ 実績がいちばん多い（electron-builder など） | ○ | ○ | △ |
| 6. 保守の状況 | ◎ 2026-11 も活発に更新 | ◎ 活発。ただしメジャー版の更新を追い続ける必要がある | ○ v3 が出たばかり | ○ | ○ |
| 7. 書きやすさ | ○ Rust は型が厳しく、コンパイラが誤りを見つけてくれる。処理（Vultr、Tailscale、資格情報）を Rust 側にまとめれば、画面（TypeScript）は薄くできる | ◎ TypeScript だけで書ける | ○ Go は単純で書きやすい | ○ | ○ |

**Tailscale の操作（どの候補でも共通）**

- **CLI を使う。** LocalAPI は Tailscale の内部の API（公式のドキュメントが無く、Go の内部パッケージの定義に依存する）なので使わない。CLI の `tailscale status --json` と `tailscale set --exit-node=...` は公式に文書化されている。
- Windows: `C:\Program Files\Tailscale\tailscale.exe`（試作で確かめた）。
- macOS: Tailscale には 3 つの種類がある（App Store 版、スタンドアロン版、オープンソースの CLI 版）。Tailscale はスタンドアロン版を推奨している。
  - App Store 版: CLI はアプリの中にあり、`/Applications/Tailscale.app/Contents/MacOS/Tailscale` で呼ぶ（公式に記載）
  - スタンドアロン版: 設定から `/usr/local/bin/tailscale` に CLI のランチャーを入れられる（macOS 13 以降、管理者のパスワードが要る）。アプリの中の CLI も呼べるかは試作で確かめる
  - アプリは `/usr/local/bin/tailscale` → アプリの中の CLI の順に探す
  - m1 の Tailscale がどの種類かは、pc-manager の記録に無い。Step 2 で m1 で確かめる
- 選んだ出口ノードが無い・出口ノードを提供していないとき、Tailscale は通信が漏れないように経路を塞ぐ（blackhole）。VPS を消す前に必ず出口を外す（試作どおり）。

**開発環境の現状（sx360 の WSL）**

- Rust（`rustc` 1.83.0、2024-11）と Node.js（v22.22.0）が入っている。Tauri 2.12 には新しい Rust が要る見込みなので、使うなら `rustup update` が要る（入れる前にユーザーに確認する）。
- Windows のターゲット（`x86_64-pc-windows-msvc`）、`cargo-xwin`、NSIS は入っていない。

**評価**

- **Tauri 2 がいちばん合う。** 要る機能（トレイ、資格情報ストア、外部コマンド、HTTP、自動更新、署名）がすべて公式か保守されているクレートでそろう。配布物が小さい。WSL に Rust と Node.js がすでにあり、Windows 版も WSL からクロスビルドできる見込み（できなければ Windows 側でビルドする）。気をつける点は、Windows のクロスビルドがあまりテストされていないこと、macOS でドックのアイコンが残る報告があること。
- **Electron は次点。** 実績と情報がいちばん多く、TypeScript だけで書ける。ただし配布物が 100 MB 前後と、トレイに常駐する小さなアプリには大きい。資格情報は `safeStorage` で暗号化して自分で保存する形になり、macOS では署名しないと動きが安定しない。メジャー版の更新を追い続ける手間もある。
- Wails v3 は Tauri に近い利点があるが、v3 の安定版が出たばかり（2026-11-01）で、トレイが安定 API の一覧に入っていない。Flutter はトレイがコミュニティのプラグイン頼み。Avalonia は資格情報ストアと macOS の配布に手間がかかる。この 3 つは見送る。

**決定（2026-11-16）**

ユーザーが **Tauri 2** に決めた。CLAUDE.md の「設計の方針」に書いた。Step 2 の試作は Tauri 2 で作り、WSL からの Windows のビルドと macOS のメニューバーなどを確かめる。

**Step 2 への提案**

- 試作は **Tauri 2 だけで作る**。Step 2 の 4 項目のどれかが Tauri でうまくいかない（とくに WSL からの Windows のビルド、macOS のメニューバー）ときに、Electron で同じ試作を作って比べる。

**出典（2026-11-16 に参照）**

- 最新版: crates.io（`tauri`、`tauri-plugin-shell`、`tray-icon`、`keyring`、`keyring-core`、`windows-native-keyring-store`、`apple-native-keyring-store`）、registry.npmjs.org（`electron`）、GitHub の wailsapp/wails の Releases、pub.dev（`tray_manager`、`flutter_secure_storage`）、nuget.org（`Avalonia`）
- Tauri の Windows のインストーラとクロスビルド: https://v2.tauri.app/distribute/windows-installer/
- Tauri のリリース: https://v2.tauri.app/release/tauri/all-versions/ 、shell プラグインと sidecar: https://v2.tauri.app/release/shell/all-versions/
- Tauri の macOS のメニューバーだけのアプリ: https://dev.to/hiyoyok/building-a-menubar-app-with-tauri-v2-what-nobody-tells-you-2nae 、ドックのアイコンが残る件: https://github.com/tauri-apps/tauri/issues/13519
- `keyring` クレート: https://docs.rs/crate/keyring/3.6.3 、https://lib.rs/crates/keyring
- Electron の `safeStorage`: https://electronjs.org/docs/latest/api/safe-storage 、リリースの周期: https://electronjs.org/docs/latest/tutorial/electron-timelines 、keytar からの移行の例: https://jira.mongodb.org/browse/COMPASS-6854
- Wails v3: https://newreleases.io/project/github/wailsapp/wails/release/v3.0.0 、https://v3.wails.io/reference/overview 、トレイ: https://docs-preview.wails.io/features/menus/systray/
- Flutter のトレイ: https://pub.dev/packages/tray_manager
- .NET の資格情報ストア: https://nuget.org/packages/ktsu.CredentialCache
- 配布物の大きさ（ブログの測定。参考値）: https://www.pkgpulse.com/guides/electron-vs-tauri-2026 、https://dev.to/gethopp/tauri-vs-electron-performance-bundle-size-and-the-real-trade-offs-1el4
- Tailscale の macOS の種類と CLI: https://tailscale.com/kb/1065/macos-variants 、CLI: https://tailscale.com/kb/1080/cli 、出口ノード: https://tailscale.com/docs/features/exit-nodes
