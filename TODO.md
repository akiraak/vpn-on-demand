# TODO

## 準備

- [~] 技術選定: Windows と macOS のアプリを 1 つのコードベースで作るフレームワークを決める [plan](docs/plans/tech-stack.md)
  - 決定: 2026-11-16 にユーザーが Tauri 2（Rust + Web の画面）に決めた。Step 2 の試作で、WSL からの Windows のビルドと macOS のメニューバーを確かめる
  - [x] Step 1: 候補を机上で比べる（トレイ、資格情報ストア、外部コマンドと HTTP、ビルド、配布、保守、書きやすさ）
    - 2026-11-16: Tauri 2 がいちばん合う、Electron が次点。Wails v3、Flutter、Avalonia は見送る（プランの「調査結果」）
  - [~] Step 2: Tauri 2 で最小のアプリを作り（だめな項目があれば Electron でも作って比べる）、sx360（Windows）と m1（macOS）で動かす（VPS は作らない）
    - 試作は `poc/tauri/` で作り、2026-11-17 に `app/` に移した。結果はプランの「Step 2: Tauri 2 の試作」
    - [x] [Windows] sx360 で確かめる（WSL からのビルド、Tailscale の状態、トレイ、資格情報ストア、リージョン、署名なしのインストール）
      - 2026-11-16: 全部できた。WSL から `cargo-xwin` と NSIS でインストーラを作れた
    - [ ] [macOS] m1 で確かめる（m1 の Claude Code で。Xcode の Command Line Tools、Rust、Node.js を入れる前にユーザーに確認する）
      - m1 の Tailscale の種類（App Store 版 / スタンドアロン版）と CLI の場所も確かめる
  - [x] Step 3: 試作の結果から、ディレクトリ構成とビルドの手順を CLAUDE.md に書き、TODO.md のタスクを Tauri に合わせて直す
    - 2026-11-18: CLAUDE.md の「開発環境」に書いた。macOS のビルドの手順は m1 で確かめてから足す
- [ ] [macOS] Tailscale の CLI をアプリから呼べるかを確かめる（App Store 版とスタンドアロン版）
  - Windows は済んだ（アプリが CLI で動いている。LocalAPI は使わないと CLAUDE.md に決めてある）
  - ほかの出口ノードを使っているときの扱いも見る

## Windows 版の仕上げ

- [ ] ドキュメントを整備する（README を読めば、だれでもインストールして使える状態にする）
  - 派生元: 「Windows 版を完成させる」（2026-11-17 に完了。[plan](docs/plans/archive/windows-app.md)）
  - README（英語）: 何ができるか、用意するもの（Vultr、Tailscale）、tailnet のポリシー、キーの作り方、インストール（SmartScreen の警告の越え方）、使い方、費用、困ったとき、片付け（アンインストール、キーの削除）
  - 配布: GitHub Actions でインストーラをビルドして下書きの Release を作り、sx360 で試作と開発版をアンインストールしてから、README だけを見て入れて使う。問題が無ければ公開する（Release の公開はユーザーに確認する）
  - 開発者向けのビルドの手順は CLAUDE.md の「開発環境」に書いた（2026-11-18）。README には要点だけ書いてリンクする

## 接続の速さ

- [ ] 接続までの時間を速くする方法を探す（根本的な技術の見直しも含める）
  - 今は「Connect」を押してから Connected まで 3〜5 分（2026-11-17 に sx360 で確かめた。[plan](docs/plans/archive/windows-app.md) の Phase 5）。VPS の作成と起動、cloud-init で Tailscale を入れて参加するまでに約 2 分かかる
  - まず、どの段階に何秒かかっているかを測る
  - 今の方式の中での改善（Tailscale を入れたスナップショットから作る、cloud-init を短くする、待ちの間隔を詰めるなど）と、方式そのものの見直し（VPS の事業者、Tailscale を使わない経路など）の両方を比べる。課金を止めることを最優先にする方針（CLAUDE.md の「設計の方針」）は変えない
  - 関連: 接続中の画面（タイムライン）は「デザインを改善する」で作った（2026-11-18 に完了。[plan](docs/plans/archive/design.md)）

## 消し忘れの対策（課金）

- [ ] アプリが動いていない間の消し忘れへの対策を検討する
  - VPS の中に Vultr の API キーを置いて自分で消させる案は避ける（Vultr の API キーは権限を絞れず、アカウント全体を操作できる）

## Tailscale まわり

- [ ] 出口ノード（`tag:vpn-on-demand`）から tailnet の中へは通さない設定を検討する
  - 今はユーザーの tailnet のポリシーが全許可なら、VPS から tailnet の機械へ届く。README で案内する grants の例を考える
- [ ] auth key の期限（最長 90 日）への対策を決める
  - 案: 設定画面で Tailscale の OAuth クライアントを入力してもらい、接続のたびに短命の auth key を発行する（期限切れが無くなり、user data に残るキーも使い捨てになる）。または期限が近いことを警告する

## 配布

- [ ] [macOS] アプリを作る。署名と公証（notarization）をするか決める（しないと Gatekeeper で開けない）
- [ ] 自動更新を入れるか決める

## pc-manager 側の後片付け（アプリができたら）

- [ ] pc-manager の記録（NordVPN の解約・アンインストールのタスクの文面、`hosts/sx360/README.md`、`hosts/m1/README.md` など）で jp-exit と書いているところを、このアプリの名前とリポジトリへのリンクに替える。sx360 と m1 にアプリを入れたことを各ホストの README に書く
  - 作業は pc-manager のリポジトリで行う
