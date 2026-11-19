# デザインを改善する（アイコン、画面の見た目、初回の案内）

## 目的・背景

「Windows 版を完成させる」（[plan](archive/windows-app.md)）で、接続・切断と消し忘れの対策は動くようになった。見た目は後回しにしていたので、公開する前に次の 3 つを直す。

- アプリのアイコンが Tauri の既定のまま（インストーラ、タスクバー、スタートメニューに出る）。トレイのアイコンは実行時に描く丸（灰 / 黄 / 緑）で、何のアプリか分からない
- 画面は動くことを優先した作りで、状態や接続中の情報が表の羅列になっている。接続には 3〜5 分かかるが、いまどこまで進んだかが文 1 行でしか分からない
- 初回の案内は、Settings タブの上にキーの入力欄、下に 4 つの手順が並び、手順の順番（Tailscale → ポリシー → auth key → Vultr）と入力欄の位置が離れている。初めての人は、上から読むと先にキーを求められて迷う

## 対応方針

### Phase 0: デザイン例を作って決める

- 2026-11-17 にユーザーが「画面のデザイン例をいくつか作って決める」と指示した
- 方向の違う案を 3 つ作り、それぞれ Connect タブ（未接続、接続中、接続済み）と Setup（初回の案内）とアイコンの例を、1 つのページ（Artifact）に並べて見比べられるようにする
- 3 案: A. Windows の小さなツール（Fluent 風、今の画面の整理）/ B. 大きな電源ボタン（一般向けの VPN アプリ風）/ C. 経路を見せる（この PC → サーバー → インターネットの絵とタイムライン）
- 見比べのページ: https://claude.ai/artifact/Pwg5wgYsgUZamw2cyyLWRQ （元のファイルはセッションのスクラッチパッド。リポジトリには入れない）
- ユーザーが選んだ案（または組み合わせ）で、下の Phase 1〜3 の中身を決め直す

- **決定（2026-11-18）: ユーザーが C（経路を見せる）に決めた。** 下の Phase 1〜3 は C に合わせて書いた

### Phase 1: アイコン

- アプリのアイコン: C の案のもの（緑の角丸の四角に、白い出発点から点線で黄色い行き先へ向かう経路）。SVG を `app/src-tauri/icons/icon.svg` に置き、ヘッドレスの Chromium で 1024 px の PNG にして、`npx tauri icon` で各サイズ（ico、icns、png）を作り直す
- トレイのアイコン: 灰色の経路と出発点に、行き先の丸を状態の色（灰 / 黄 / 緑）で塗ったもの。SVG から 32 px の PNG を 3 つ作り（`app/src-tauri/icons/tray/`）、`include_bytes!` で埋め込む。実行時に描く処理は消す

### Phase 2: Connect タブ

- タブ（Connect / Settings）は下線で示す
- 上に経路の絵: This PC → サーバー（都市の名前）→ Internet。状態で絵を変える
  - 未接続: サーバーは点線の枠（No server）、下に「Traffic goes out from your own connection」
  - 接続中・切断中: サーバーは黄色の点線の枠で点滅、線は黄色
  - 接続済み: 全部緑の線
  - 残った VPS がある: サーバーは黄色で「Still running」。Delete it / Use it のボタンを出す
- 未接続のとき: 国の一覧（検索つき）。選んだ国の行に都市を並べ、複数あれば選べる。下に Connect のボタン（「Connect via Tokyo」）
- 接続中・切断中: 時刻つきのタイムライン。Rust の `Step` を手順の番号に対応させ、手順が変わった時刻を画面側で覚えて出す
  - 接続: Check Tailscale and Vultr → Create a server（`removing_other` / `using_existing` もここ）→ Join your tailnet（約 2 分）→ Approve as an exit node（待っているときは管理画面への案内）→ Switch this PC → Check the public IP
  - 切断: Turn off the exit node → Delete the server → Check that it's gone
- 接続済み: 外の IP の行（選んだ国と違えば警告）、タイル 3 つ（Running、Cost so far、Auto-off in と Reset）、Disconnect のボタン
- エラーやお知らせの帯は、経路の絵の上に出す

### Phase 3: Settings タブと初回の案内

- 上に「Setup checklist」（N of 4 done）。4 つの行を開いて済ませる
  1. Tailscale is installed and signed in: Tailscale の状態で自動で ✓（`UiState.tailscale_error` が無い）
  2. Tailnet policy allows the app's servers: アプリから確かめられないので、ポリシーをコピーするボタンと「Mark as done」を置く。済んだ印は設定（`Settings.policy_done`、新しく足す）に保存する
  3. Create a Tailscale auth key: 手順と入力欄。保存されていれば ✓ と伏せたキー、Replace / Remove
  4. Create a Vultr API key: 同じ
- 済んでいない最初の行を開いておく。全部済んだら行は閉じ、Test setup のボタン。結果は各行の印（Checked / Works / エラー）に出す。初回で全部 ✓ なら「Go to Connect」を出す
- その下に Auto-disconnect と、バージョン・GitHub へのリンク
- 終了の確認の画面も、ほかの画面と見た目をそろえる

### Phase 4: sx360 で確かめる

- WSL から Windows 版をビルドし、sx360 で開発版を入れ直して、アイコン（インストーラ、タスクバー、スタートメニュー、トレイ）と画面を確かめる
- 初回の案内は、キーを一度消した状態で試す（消す前にユーザーに確認する。キーはユーザーが入れ直す）

## 影響範囲

- `app/index.html`、`app/src/main.ts`、`app/src/styles.css`
- `app/src-tauri/icons/`（作り直し）、`app/src-tauri/src/tray.rs`（アイコンを埋め込みの PNG に）
- `app/src-tauri/src/settings.rs`（`policy_done` を足す）
- Rust のロジック（接続・切断）は変えない。画面に渡す状態（`UiState`）も、足りなければ足すだけにする
- VPS は作らない。接続中の画面は偽物の状態で確かめる（実物で確かめたくなったら、作る前にユーザーに確認する）

## テスト方針

- 画面: Tauri の API（`window.__TAURI_INTERNALS__`）を偽物に差し替えるページをスクラッチパッドに作り（リポジトリには入れない）、ヘッドレスの Chromium で各状態（起動中、キー無し、未接続、残った VPS、接続中の各手順、承認待ち、接続済み、切断中、エラー、終了の確認、ダークモード、最小の窓の大きさ 420×520）を撮って確かめる
- `npm run build`（tsc）と `cargo test`、`cargo clippy` が通ること
- sx360 で実物を確かめる（Phase 4）。フォントは Windows（Segoe UI）で見え方が変わるので、最終の確認は sx360 の画面で行う

## 進み具合

### Phase 1〜3（2026-11-18）

- アイコン: `icons/icon.svg` から 1024 px の PNG を作り、`npx tauri icon` で作り直した（Android と iOS 用に作られたものは消した）。トレイは `icons/tray/{off,busy,on}.svg` から 32 px の PNG を作り、`tray.rs` で `include_bytes!` で埋め込んだ（実行時に描く処理は消した）。埋め込んだ PNG が読めるかの単体テストを足した
- **アイコンを替えたときは `src-tauri/build.rs` を `touch` してからビルドする。** アイコンのファイルを替えただけでは、exe に埋め込むビルドスクリプトがやり直されず、古いアイコンのままになった
- 画面: `index.html`、`styles.css`、`main.ts` を C の案で書き直した。Connect（経路の絵、検索つきの国の一覧と都市のボタン、時刻つきのタイムライン、接続中の IP とタイル）、Settings（開いて済ませるチェックリスト。Test setup の結果を各行の印に出す）、終了の確認の画面。ダークモードは Windows の設定に合わせる
- `Settings.policy_done` を足した。前の版の設定ファイル（この項目が無い）は、すでに設定を済ませた人のものなので「済んだ」とみなす（`#[serde(default = "yes")]`）。設定ファイルが無い新しい人は「済んでいない」から始まる
- 確かめたこと: Tauri の API を偽物に替えたページをヘッドレスの Chromium で撮って、未接続、検索、残った VPS、エラー、接続中、承認待ち、Cancel、接続済み、IP の国の食い違い、切断中、初回（Tailscale 無しを含む）、設定済み、終了の確認を、ライト・ダーク・最小の窓で確かめた（撮影のスクリプトはスクラッチパッドに置き、リポジトリには入れていない）。`npm run build`、`cargo test`（26 件）、`cargo clippy` は通った

### Phase 4（2026-11-18、sx360）

- WSL から Windows 版をビルドし、sx360 に入れ直した（インストーラ 2.26 MiB）。exe と窓のアイコンが新しいものになったこと、Connect と Settings の画面（Windows はダークモード）、Test setup の結果の出方を、UI Automation と PrintWindow で確かめた
- 前の版の設定ファイルのまま起動して、チェックリストが「4 of 4 done」になることを確かめた
- まだ: トレイ、タスクバー、スタートメニュー、インストーラのアイコンの見た目（ユーザーに画面で見てもらう）。VPS を作る接続・切断の画面は偽物の状態だけで確かめた
