# CLAUDE.md

このファイルは、このリポジトリで作業する Claude Code へのガイダンスです。

## プロジェクトの目的

**使うときだけ VPS を立てる個人用の VPN の、Windows と Mac のデスクトップアプリ**（On-demand personal VPN）。

- アプリで国を選んで「接続」すると、Vultr のその国のリージョンに VPS を作り、Tailscale の出口ノードにして、その PC の外向きの通信をその国から出す。「切断」で出口を外して VPS を消す。
- Vultr の API キーと Tailscale のキーは、アプリの**設定画面**で入力する。
- 費用は使った時間の分だけ（Vultr の最小構成で約 $0.007/時、使わない月は 0）。

主な用途は、日本の外から日本でしか配信していない動画サイト（Amazon Prime Video（日本）、TVer、U-NEXT）を見ること。NordVPN の代わりとして作る。日本以外の国も選べるようにする。

## 経緯

- pc-manager（`~/pc-manager`、`git@github.com:akiraak/pc-manager.git`）の「NordVPN に代わる VPN の仕組みの調査」で、東京の VPS に Tailscale の出口ノードを立てる試作 `jp-exit`（Python の CLI）を作った。2026-11-16 に sx360 から Prime Video、TVer、U-NEXT の 3 つとも見られることを確かめ、NordVPN をやめてこの方式に乗り換えると決めた。
  - 調査プラン: `~/pc-manager/docs/plans/archive/vpn-alternatives.md`（候補の比較、試験の結果、出典）
- 同じ日に、ツール化をこのリポジトリへ移すことにし、pc-manager からは試作を消した。2026-11-16 にユーザーが、CLI ではなく Windows と Mac のアプリにすると決めた。
- 試作の中身は pc-manager の Git 履歴で読める。アプリの処理（Vultr の API、cloud-init、Tailscale の操作）の参考にする。

  ```bash
  git -C ~/pc-manager show 7a5c0b1:tools/jp-exit/sx360/jp-exit.py
  git -C ~/pc-manager show 7a5c0b1:tools/jp-exit/README.md
  ```

- pc-manager 側に残っているのは、NordVPN の解約と m1 のアンインストールのタスクだけ。PC の設定（NordVPN の削除など）は pc-manager で扱い、このリポジトリではアプリだけを作る。

## 試作（jp-exit）のしくみ

アプリの「接続」「切断」の処理の出発点。

- 「接続」（試作の `up`）: Vultr API v2 で `nrt`（東京）に `vc2-1c-1gb`（無ければ IPv4 付きで一番安い vc2）の Ubuntu 24.04 を作る。ラベルとホスト名は `jp-exit`。同じラベルの VPS があれば作らずに使う。
- cloud-init の user data（bash）で、IP 転送を有効にし、Tailscale を入れて `tailscale up --auth-key=... --hostname=jp-exit --advertise-exit-node --ssh` で参加する。公開の 22 番は ufw で閉じる（SSH は Tailscale SSH だけ）。作ってから参加まで約 2 分。
- Tailscale にオンラインになるのを待ち、出口ノードとして承認される（`tailscale status --json` のピアの `ExitNodeOption`）のを待ってから、`tailscale set --exit-node=jp-exit --exit-node-allow-lan-access=true` で切り替える。
- 外の IP アドレスと国は `https://ipinfo.io/json` で確かめた。
- 「切断」（試作の `down`）: `tailscale set --exit-node=` で出口を外し、ラベルで見つけた VPS を消す。
- auth key は Reusable + Ephemeral。VPS を消すと tailnet からも自動で消える。
- ローカルに状態ファイルは作らない。VPS の有無は Vultr の API（ラベルで探す）で見る。

## 試作で分かったこと（アプリで必ず扱う）

- **VPS を作るたびに Tailscale では新しいマシンになり、管理画面で出口ノードの承認が要る。** tailnet のポリシーの `autoApprovers` の `exitNode` で省く。auth key にタグを付け、そのタグだけを承認するのが安全。ポリシーの編集はユーザーが管理画面で行うので、アプリの設定画面や README で手順を案内する。
- **出口を外した直後、通信が数秒つながらないことがある**（試作は WSL の mirrored ネットワークで、VPS の削除が `Network is unreachable` で失敗した）。Vultr の API 呼び出しは、つながらないときに再試行する。
- **Vultr は電源を切ったインスタンスにも課金する。** 止めるときは消す。消し忘れると時間課金が続くので、アプリで消し忘れを防ぐ。
- **auth key は cloud-init の user data に入り、VPS のメタデータ（VPS の中から読める）に残る。** auth key には期限（最長 90 日）があり、切れると VPS が Tailscale に参加しなくなる。
- データセンターの IP でも、2026-11-16 の時点では 3 サイトとも止められなかった。サイトが判定を強めれば止められる可能性はある。そのときは VPS を作り直して IP を変える。

## 設計の方針

- **対象は Windows と macOS のデスクトップアプリ。** 1 つのコードベースから両方を作る。**フレームワークは Tauri 2**（2026-11-16 に決めた。比較は `docs/plans/tech-stack.md`）。処理（Vultr の API、Tailscale の CLI、資格情報ストア、タイマー）は Rust 側にまとめ、画面（Web、TypeScript）は表示と操作だけにする。資格情報ストアは `keyring` クレートを使う。
- **画面の言語は英語**（2026-11-16 に決めた）。メニュー、ボタン、メッセージ、Rust 側から画面に返すエラーの文言も英語にする。
- **画面**: 国を選んで接続・切断するメイン画面（状態、外の IP アドレスと国、接続してからの時間と概算の料金）と、キーを入力する設定画面。常駐してタスクトレイ / メニューバーから操作できるようにする。
- **国の選択**: Vultr の `/regions` のリージョンを国ごとにまとめて出す。1 つの国に複数のリージョンがあれば（米国など）都市も選べるようにする。既定は日本（`nrt`）。
- **課金を止めることを最優先にする。** 消し忘れを防ぐ仕組み（一定時間で自動で切断、アプリを終了するときに VPS が残っていれば警告など）を持つ。切断は失敗しても再試行でき、何度実行しても安全（冪等）にする。VPS はアプリ専用のラベル / タグで見つけ、ローカルの状態に頼らない（アプリが落ちても、次に起動したときに残った VPS を見つけて消せる）。
- **Tailscale は前提。** 利用者の PC に Tailscale のアプリが入っていて、tailnet にログインしていることを前提にし、アプリは Tailscale の CLI（または LocalAPI）で出口ノードを切り替える。Tailscale が無い・ログインしていないときは案内を出す。
  - Windows: `C:\Program Files\Tailscale\tailscale.exe`
  - macOS: `/usr/local/bin/tailscale`（スタンドアロン版で CLI のランチャーを入れた場合）→ `/Applications/Tailscale.app/Contents/MacOS/Tailscale`（App Store 版はここ）の順に探す
  - LocalAPI は内部の API で文書化されていないので使わない
- **専用の VPN クライアントやドライバは入れない。** 通信の経路は Tailscale に任せる。

## 秘密情報

- 利用者のキー（Vultr の API キー、Tailscale の auth key など）は、アプリの設定画面で入力し、**OS の資格情報ストア**（Windows の資格情報マネージャー、macOS のキーチェーン）に保存する。設定ファイルに平文で書かない。ログや画面にも全文を出さない（伏せて表示する）。
- 設定画面で入力するキー（試作で使ったもの）:
  - Vultr の API キー（Vultr の Account → API。Access Control で使う PC の外の IP アドレス、または全部を許可する）
  - Tailscale の auth key（Settings → Keys。Reusable と Ephemeral をオン。タグを付ける）。OAuth クライアントに替えるかは Tailscale のタスクで決める
- 開発中にテストで使うキーは、リポジトリ直下の `.env`（Git 管理外）に置き、変数名だけを `.env.example` に書いてコミットする。値はユーザーが入れる。Claude Code は `.env` を `cat` や Read で表示せず、コマンドの引数、出力、記録、コミットにも写さない。

## 開発環境

- Claude Code は sx360（Windows 11 + WSL2）の WSL で動く。Windows 版のビルドと動作確認は Windows 側で行う必要があるかもしれない（フレームワークによる）。WSL から Windows のコマンドを呼ぶときは `cd /mnt/c` してから `powershell.exe` を使う（詳しくは `~/pc-manager/CLAUDE.md` の「Windows ホストでの操作方法」）。
- macOS 版のビルドと動作確認は m1 で行う。m1 での作業は m1 の Claude Code で行う。

## 注意が必要な操作

次の操作は、実行前に必ずユーザーへ確認すること。

- **VPS を作る**（課金が始まる）。作ったら、作業の終わりに消したかを必ず確かめる。
- Tailscale の出口ノードの切り替え（その PC の全通信の経路が変わる。WSL や SSH が一時的に切れることがある）。
- Tailscale の tailnet のポリシー（ACL、`autoApprovers`、タグ）の変更の提案。編集はユーザーが管理画面で行う。
- 開発用のツールチェーン（Rust、Node.js、Xcode、Visual Studio の Build Tools など）を Windows や macOS に入れる。

調査（読み取りだけの API 呼び出し、`tailscale status` など）は確認なしで進めてよい。

## Git

- リモートは `git@github.com:akiraak/vpn-on-demand.git`。
- コミットメッセージは日本語で、何を変えたかを 1 行目に書く。
- 秘密情報はコミットしない。

## 開発管理画面

タスク・プラン・ファイルの閲覧と編集は project-manager（`http://127.0.0.1:8200`、sx360 では `~/project-manager`）で行う。下のタスク管理ルールは、その Tasks タブと Plans タブの読み方に合わせている。

## タスク管理ルール

- タスクは `TODO.md` で管理する
- **タスクの行は、親項目も含めて必ず `- [ ] 文面` の形で書く。** project-manager の Tasks タブはチェックボックス
  （`[ ]` 未着手 / `[x]` 完了 / `[~]` 進行中 / `[-]` 中止）のある行だけをタスクとして拾う。
  チェックボックスの無い行は直前のタスクのメモ扱いになる
- **`TODO.md` に書くのはタスクだけ。** メモや決定事項は、関係するタスクの下に字下げして付ける。
  プロジェクトとしての決定は `CLAUDE.md` へ、済んだ経緯は `DONE.md` へ書く
- 字下げが親子。Tasks タブはこれをツリーとして表示する
- タスク同士の関係は、そのタスクの下に字下げした **`依存:` / `派生元:` / `関連:`** の行で書く。
  相手のタスクは `「文面」` で、プランや仕様は Markdown リンクで示す
- タスクの期日は、そのタスクの下に字下げした **`期日:`** の行で書く（例: `期日: 2026-11-20`）
- 特定の OS でしか作業できないタスクには、文面の先頭に `[Windows]` / `[macOS]` を付ける
- タスクが完了したら `TODO.md` から該当項目を削除し、`DONE.md` に完了日（`YYYY-MM-DD`）を付けて移す
- 新しいタスクが発生したら `TODO.md` の適切なセクションに追加する
- タスクの実施前に `TODO.md` を確認し、優先度の高いものから着手する

## 作業着手ルール

作業（実装・調査いずれも）を始めるときは、コードに手を入れる前に以下を行う。

1. **プランファイルを作成する**: `docs/plans/<task-name>.md` に、目的・背景、対応方針、影響範囲、テスト方針を書く。複数の Phase / Step に分かれる場合はファイル内でも明示する
2. **`TODO.md` の該当項目にプランへのリンクを足す**（例: `[plan](docs/plans/<task-name>.md)`）。無ければ項目を追加する
3. **複数の Phase / Step がある場合は `TODO.md` に子タスクとして追加する**。完了するごとにチェックを入れる
4. **作業完了時**: 親タスクを `DONE.md` に移し、プランを `docs/plans/archive/` に移す
