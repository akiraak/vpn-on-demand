# TODO

## 準備

- [~] 技術選定: Windows と macOS のアプリを 1 つのコードベースで作るフレームワークを決める [plan](docs/plans/tech-stack.md)
  - 決定: 2026-11-16 にユーザーが Tauri 2（Rust + Web の画面）に決めた。Step 2 の試作で、WSL からの Windows のビルドと macOS のメニューバーを確かめる
  - [x] Step 1: 候補を机上で比べる（トレイ、資格情報ストア、外部コマンドと HTTP、ビルド、配布、保守、書きやすさ）
    - 2026-11-16: Tauri 2 がいちばん合う、Electron が次点。Wails v3、Flutter、Avalonia は見送る（プランの「調査結果」）
  - [ ] Step 2: Tauri 2 で最小のアプリを作り（だめな項目があれば Electron でも作って比べる）、sx360（Windows）と m1（macOS）で動かす（VPS は作らない）
  - [ ] Step 3: 試作の結果から、ディレクトリ構成とビルドの手順を CLAUDE.md に書き、TODO.md のタスクを Tauri に合わせて直す
- [ ] 開発環境を作る（選んだフレームワークのツールチェーンを sx360 の Windows / WSL と m1 に入れる）
  - 依存: 「技術選定: Windows と macOS のアプリを 1 つのコードベースで作るフレームワークを決める」
  - WSL からビルドできるか、Windows 側でビルドするかを確かめる
- [ ] Tailscale の操作方法を調べる: Windows と macOS（App Store 版とスタンドアロン版）で、アプリから CLI（`tailscale set --exit-node`、`tailscale status --json`）を呼べるか、LocalAPI を使う方がよいか
  - Tailscale が入っていない・ログインしていない・ほかの出口ノードを使っているときの見分け方も調べる

## コア処理（試作 jp-exit からの移植）

- [ ] Vultr の API の処理: リージョンの一覧（国ごとにまとめる）、プランの選択（そのリージョンで一番安い IPv4 付き）、VPS の作成・一覧・削除
  - 試作: `git -C ~/pc-manager show 7a5c0b1:tools/jp-exit/sx360/jp-exit.py`（しくみは CLAUDE.md の「試作（jp-exit）のしくみ」）
  - アプリ専用のラベル / タグを付け、それで見つける（ほかの VPS に触れない）
  - つながらないときは再試行する
- [ ] cloud-init の user data（IP 転送、Tailscale を入れて出口ノードとして参加、公開の 22 番を閉じる）
- [ ] 接続の流れ: VPS を作る → Tailscale にオンラインになるのを待つ → 出口ノードの承認を待つ → 出口を切り替える → 外の IP アドレスの国が選んだ国と合っているかを確かめる
  - 各段階の進み具合を画面に出す。承認待ちのときは、管理画面での操作を案内する
- [ ] 切断の流れ: 出口を外す → VPS を消す → 消えたことを API で確かめる。失敗しても再実行で確実に消せる（冪等）
- [ ] テスト: Vultr の API と Tailscale の操作を差し替えて、接続・切断・異常系（作成の失敗、参加のタイムアウト、削除の失敗）を確かめる

## 画面

- [ ] メイン画面: 国（と都市）を選んで接続・切断。状態、外の IP アドレスと国、接続してからの時間と概算の料金を出す
- [ ] 設定画面: Vultr の API キーと Tailscale のキーを入力し、OS の資格情報ストアに保存する
  - 保存したキーは伏せて表示する。「接続を確かめる」ボタンで、キーが使えるか（Vultr の API が通るか、Tailscale が動いているか）を確かめる
  - キーの作り方（Vultr の API と Access Control、Tailscale の auth key の Reusable・Ephemeral・タグ）と、`autoApprovers` の設定を案内する
- [ ] タスクトレイ（Windows）/ メニューバー（macOS）に常駐し、そこから接続・切断できるようにする。接続中はアイコンで分かるようにする

## 消し忘れの対策（課金）

- [ ] 一定時間（設定で変えられる。例: 4 時間）で自動で切断する
- [ ] アプリの起動時に、残っている VPS（アプリのラベル / タグの付いたもの）を見つけて、消すか使い続けるかを聞く
- [ ] アプリを終了するとき、VPS が残っていれば警告する（消してから終了 / 残して終了）
- [ ] アプリが動いていない間の消し忘れへの対策を検討する
  - VPS の中に Vultr の API キーを置いて自分で消させる案は避ける（Vultr の API キーは権限を絞れず、アカウント全体を操作できる）

## Tailscale まわり

- [ ] 出口ノードを毎回承認しなくて済むようにする（tailnet のポリシーの `autoApprovers` の `exitNode`）
  - auth key にタグ（例: `tag:vpn-exit`）を付け、そのタグだけを承認する。`tagOwners` の追加も要る
  - ポリシーの編集はユーザーが Tailscale の管理画面の Access controls で行う。足す内容を設定画面と README で案内する
  - 出口ノードから tailnet の中へは通さない設定も検討する
- [ ] auth key の期限（最長 90 日）への対策を決める
  - 案: 設定画面で Tailscale の OAuth クライアントを入力してもらい、接続のたびに短命の auth key を発行する（期限切れが無くなり、user data に残るキーも使い捨てになる）。または期限が近いことを警告する

## 配布

- [ ] [Windows] インストーラを作る。コード署名をするか決める（しないと SmartScreen の警告が出る）
- [ ] [macOS] アプリを作る。署名と公証（notarization）をするか決める（しないと Gatekeeper で開けない）
- [ ] README を書く（何ができるか、前提（Vultr のアカウント、Tailscale）、キーの作り方、費用の目安、「使い終わったら切断」）
- [ ] 自動更新を入れるか決める

## pc-manager 側の後片付け（アプリができたら）

- [ ] pc-manager の記録（NordVPN の解約・アンインストールのタスクの文面、`hosts/sx360/README.md`、`hosts/m1/README.md` など）で jp-exit と書いているところを、このアプリの名前とリポジトリへのリンクに替える。sx360 と m1 にアプリを入れたことを各ホストの README に書く
  - 作業は pc-manager のリポジトリで行う
