# Windows 版を完成させる

## 目的・背景

2026-11-16 にユーザーが「Windows アプリを完成させる」と決めた。技術選定の試作（`poc/tauri/`）は、Tailscale の状態を読む、ダミーのキーを保存する、リージョンを出すだけで、VPS を作ることも出口ノードを切り替えることもできない。これを、誰でも使える Windows アプリにして公開する。

**完成の条件: 誰でも GitHub のドキュメント（README）を読んで、アプリをインストールして使えること**（2026-11-16 にユーザーが決めた）。リポジトリ `akiraak/vpn-on-demand` は公開済み（MIT）。

- README（英語）だけを読めば、Vultr と Tailscale のアカウントを作るところから、キーの作り方、tailnet のポリシーの編集、アプリのインストール、使い方、費用、片付け（アンインストール、キーの取り消し）までできる
- GitHub の Releases から Windows のインストーラを落として入れられる。インストーラは GitHub Actions でビルドする（誰がビルドしても同じものになり、手元の WSL に頼らない）
- アプリの中にも、初めて起動したときの案内（何を用意し、どこで何を入れるか、リンク付き）がある。Tailscale が無い・ログインしていない・出口ノードが承認されていない、などのときは、次に何をすればよいかを出す
- 特定の人の環境（sx360、akiraak の tailnet）を前提にしない

そのうえで、sx360 で次を確かめる:

- 国（と都市）を選んで「Connect」で、その国から外へ出られる。「Disconnect」で出口が外れ、VPS が消える
- キーは設定画面で入れ、資格情報マネージャーに保存される
- タスクトレイに常駐し、トレイから接続・切断できる
- 消し忘れを防ぐ（一定時間で自動で切断、起動時に残った VPS を見つける、終了時に警告）
- 画面は英語（CLAUDE.md の「設計の方針」）
- Releases のインストーラで sx360 に入れて使える（README の手順どおりに、まっさらな状態から）

macOS（m1）の確認は後回しにする。コードは macOS でも動くように書き（OS ごとの処理は `cfg` で分ける）、TODO の「[macOS] m1 で確かめる」は残す。

## 対応方針

### 構成

- 試作 `poc/tauri/` を `app/` に移して本体の出発点にする（技術選定のプランの「決めた候補の試作は本体の出発点にしてよい」）。`poc/` は消す
  - 製品名 `VPN on Demand`、identifier `io.github.akiraak.vpn-on-demand`（公開するので、リポジトリに合わせた逆ドメイン名にする。試作と別なので、試作のアンインストールと混ざらない）
  - 版は `0.1.0` から始める
- Rust 側（`app/src-tauri/src/`）
  - `vultr.rs`: Vultr API v2 のクライアント
  - `tailscale.rs`: Tailscale の CLI
  - `cloud_init.rs`: user data（bash）を作る
  - `vpn.rs`: 接続・切断の流れ（Vultr と Tailscale はトレイトで差し替えられるようにし、テストで偽物を使う）
  - `secrets.rs`: 資格情報ストア（試作のまま、英語に）
  - `settings.rs`: 秘密でない設定（自動切断までの時間、最後に選んだリージョン）を、アプリの設定フォルダの JSON に保存
  - `lib.rs`: Tauri のコマンド、トレイ、タイマー、終了時の処理
- 画面（`app/src/`）: 表示と操作だけ。状態は Rust 側が持ち、イベントで画面に送る

### 接続（Connect）

1. キーがそろっているか、Tailscale が動いているか（`BackendState` が `Running`）を確かめる
2. アプリのタグ（`vpn-on-demand`）の付いた VPS を探す。あれば作らずに使う（別のリージョンなら、消してから作り直すかを聞く）
3. 無ければ作る
   - プラン: そのリージョンの `/regions/{id}/availability` にあり、`-v6`（IPv4 なし）と `-free` を除いた vc2 で、時間単価（`location_cost` があればそれ）が一番安いもの
   - OS: `/os` で名前が `Ubuntu 24.04 LTS x64` のもの
   - ラベルとホスト名は `vpn-on-demand-<region>`（例 `vpn-on-demand-nrt`）、タグ `vpn-on-demand`
   - user data: 試作と同じ（IP 転送、Tailscale を入れて `tailscale up --auth-key=... --hostname=... --advertise-exit-node --ssh`、ufw で 22 番を閉じる）
4. Tailscale にそのホスト名のピアがオンラインになるのを待つ（最長 10 分）。消えたばかりの同じ名前の ephemeral ノードが残っていることがあるので、オンラインのものを選び、切り替えには名前でなく Tailscale の IP アドレスを使う
5. 出口ノードとして承認される（`ExitNodeOption`）のを待つ（最長 10 分）。待っている間、管理画面での承認と `autoApprovers` の設定を案内する
6. `tailscale set --exit-node=<ホスト名> --exit-node-allow-lan-access=true`
7. `https://ipinfo.io/json` で外の IP アドレスと国を確かめ、選んだ国と違えば警告する

各段階を画面に出す（Creating server / Waiting for Tailscale / Waiting for exit node approval / Switching / Connected）。途中で「Cancel」できる（作った VPS は消す）。

### 切断（Disconnect）

1. `tailscale set --exit-node=`（使っていなければ何もしない）
2. タグで見つけた VPS を全部消す。つながらないとき（出口を外した直後の数秒など）は再試行する
3. 一覧を取り直し、消えたことを確かめる
4. 失敗しても、もう一度押せば同じ流れをやり直せる（冪等）

### 消し忘れの対策

- 自動切断: 接続してから一定時間（既定 4 時間、設定で 1〜24 時間。無効にもできる）で切断する。10 分前にトレイの通知で知らせる
- 起動時: タグの付いた VPS があれば知らせ、「Disconnect（消す）」か「Use it（使い続ける）」を選ばせる
- 終了時: VPS が残っていれば「Delete and quit / Quit and keep it running / Cancel」を聞く
- 二重起動を防ぐ（`tauri-plugin-single-instance`）。2 つ目を起動したら 1 つ目の窓を出す
- アプリが動いていない間の対策は、このプランでは扱わない（TODO に残す）

### 画面（英語）

- Main: 国と都市の選択（既定は日本 `nrt`、最後に選んだものを覚える）、Connect / Disconnect、進み具合、接続中は外の IP アドレスと国、経過時間、概算の料金（時間単価 × 経過時間）、自動切断までの残り時間
- Settings: Vultr の API キー、Tailscale の auth key（伏せて表示、上書き、消す）、「Test」（Vultr の `/account` が通るか、Tailscale が動いているか）、自動切断の時間、キーの作り方と `autoApprovers` の案内
- トレイ: アイコンで接続中かが分かる（切断中は灰色、接続中は色付き）。メニューは Open / Connect（最後に選んだ国）/ Disconnect / Quit。左クリックで窓を出す

### 他人の環境で起きること（誰でも使えるようにするため）

- **Vultr の API キーの Access Control**: 出口ノードを使っている間、PC から Vultr への通信は VPS の IP アドレスから出る。Access Control を自宅の IP アドレスだけにしていると、接続中の API 呼び出し（状態の確認、起動時の検出）が拒まれる。切断は先に出口を外すので通る。README では「Allow all IPv4 / IPv6」または自宅の IP アドレスを案内し、拒まれたとき（HTTP 401 / 403）はその理由を画面に出す
- **tailnet のポリシー**: auth key にタグを付けるには、ポリシーの `tagOwners` にそのタグが要る。README に、`tagOwners` と `autoApprovers` に足す JSON を載せる（出口ノードからほかの機械へは通さない `grants` / `acls` の例も検討する）
- **Tailscale のインストール先**: 既定の場所（`C:\Program Files\Tailscale\tailscale.exe`）に無ければ、案内とダウンロードのリンクを出す
- **Vultr の新しいアカウント**: 作れるインスタンスの数や、支払い方法の登録が要ることがある。作成のエラーは Vultr の応答の文言をそのまま（キーは含めずに）画面に出す

### 配布

- GitHub Actions（`windows-latest`）で、タグ（`v*`）を push したときに NSIS のインストーラをビルドし、下書きの Release に添付する（`tauri-apps/tauri-action`）。下書きをユーザーが確かめてから公開する
- コード署名はしない（証明書は年に数万円かかる）。SmartScreen の「Windows によって PC が保護されました」→「詳細情報」→「実行」の手順を README に画像付きで書く。署名をするかは TODO の「インストーラを作る。コード署名をするか決める」で別に決める
- 自動更新は入れない（TODO の「自動更新を入れるか決める」で別に決める）。README に、新しい版は Releases から入れ直すと書く

### キーと秘密情報

- auth key は試作と同じく Reusable + Ephemeral + タグ。OAuth クライアントに替えるかは、TODO の「auth key の期限への対策を決める」で別に扱う
- キーはログや画面に全文を出さない。Vultr の API のエラーを画面に出すときも、キーを含めない

## Phase

- Phase 1: 試作を `app/` に移し、英語にして、構成を整える（`poc/` は消す）
- Phase 2: Rust のコア（Vultr、cloud-init、Tailscale、接続・切断の流れ）と、偽物を使ったテスト
- Phase 3: 画面（Main、Settings）とトレイ
- Phase 4: 消し忘れの対策（自動切断、起動時、終了時、二重起動）
- Phase 5: sx360 で本物の VPS を使って確かめる（**VPS を作る前と、出口ノードを切り替える前に、ユーザーに確認する**。終わったら VPS が消えたことを API で確かめる）
- Phase 6: README（英語。用意するもの、キーの作り方、tailnet のポリシー、インストール、使い方、費用、困ったとき、片付け）と、アプリの中の初回の案内を仕上げる。CLAUDE.md の「開発環境」にビルドの手順を書く
- Phase 7: GitHub Actions でインストーラをビルドし、下書きの Release を作る。sx360 で試作と開発版をアンインストールし、README だけを見て Release から入れて通しで使う。問題が無ければ Release を公開する（**Release の公開はユーザーに確認する**）

## 影響範囲

- このリポジトリ: `app/`（新規）、`poc/`（消す）、`.github/workflows/`（新規）、CLAUDE.md、README.md、`docs/`（README の画像など）、TODO.md、技術選定のプラン（Step 3 の一部をここで行う）
- GitHub: Actions の実行、Release（下書き → 公開）
- sx360: 試作をアンインストールし、本体を入れる。Phase 5 で Vultr に VPS を作り（数十分で数セント）、出口ノードを切り替える
- Vultr のアカウントと tailnet: 作った VPS は必ず消す

## テスト方針

- Rust の単体テスト: Vultr と Tailscale をトレイトの偽物に替えて、接続・切断・異常系（作成の失敗、参加のタイムアウト、承認のタイムアウト、削除の失敗と再試行、VPS が既にある、Cancel）を確かめる。プランの選び方、user data、IP の国の判定も確かめる
- WSL で `cargo test`、Windows 向けのクロスビルドが通ること
- Phase 7 で、README だけを見て、まっさらな状態（アプリ無し、設定無し）から入れて使えるかを確かめる。README で分かりにくいところは直す
- Phase 5 で sx360 で通しで確かめる: 接続 → Prime Video / TVer が見られるか（ユーザー）→ 切断 → VPS が消えた、Tailscale の出口が外れた。起動時の残った VPS の検出、終了時の警告、自動切断（時間を短くして）も確かめる

## 進み具合

### Phase 1〜4（2026-11-16）

- `poc/tauri/` を `app/` に移した（`git mv`）。製品名 `VPN on Demand`、identifier `io.github.akiraak.vpn-on-demand`、Cargo のパッケージ名 `vpn-on-demand`。NSIS は `installMode: currentUser`（管理者の権限なしで入る）
- Rust: `error.rs`、`cloud_init.rs`、`vultr.rs`（再試行、HTTP 401 / 403 のときに Access Control を案内）、`tailscale.rs`、`vpn.rs`（`Cloud` / `Mesh` / `Probe` のトレイトと、接続・切断の流れ）、`secrets.rs`（サービス名 `vpn-on-demand`）、`settings.rs`、`tray.rs`（状態ごとの丸いアイコンを実行時に描く）、`lib.rs`（状態、コマンド、自動切断、外で出口が外されたときの検出、起動時の検出、終了時の確認、二重起動の防止、通知）
- 追加した依存: `tokio`、`base64`、`tauri-plugin-single-instance`、`tauri-plugin-notification`、`tauri-plugin-opener`
- 単体テスト 23 件（偽物を使った接続・切断・異常系 13 件を含む）。`cargo clippy` の警告なし
- WSL から Windows 版をビルドし（インストーラ 2.27 MiB）、sx360 で exe を起動して、キーが無いときに設定画面と案内が出ること、メイン画面、Test setup（Tailscale は ✓、キーは未設定）を確かめた。画面の操作と撮影は PowerShell の UI Automation と PrintWindow で行った（ほかの窓が写らない）
- 自動切断の時間は、環境変数 `VOD_TEST_AUTO_DISCONNECT_SECS`（秒）で短くできる（テスト用）

### Phase 5（2026-11-17、sx360）

ユーザーが tailnet のポリシーに `tagOwners`（`tag:vpn-on-demand`）と `autoApprovers.exitNode` を足し、タグ付きの Reusable + Ephemeral の auth key を作り、Vultr の API キーの Access Control を全部許可にしてから、アプリの Settings に 2 つのキーを入れた。Test setup は 3 つとも ✓。VPS は 5 回作り、毎回、終わった後に Vultr の API で 0 台になったことを確かめた（この試験の費用は約 $0.06）。

| 確かめたこと | 結果 |
|---|---|
| 接続: VPS の作成（nrt、vc2-1c-1gb、ラベル `vpn-on-demand-nrt`、タグ `vpn-on-demand`）→ tailnet への参加 → 自動承認 → 出口の切り替え → 外の IP アドレスが JP | ◎（作ってから Connected まで 3〜5 分） |
| 切断: 出口を外す → VPS を消す（トレイのメニューから） | ◎ |
| トレイ: アイコンの色（灰 / 黄 / 緑）、メニュー（Connect / Disconnect / Quit） | ◎ |
| 終了の確認の画面: 「Quit and keep it running」→ 起動し直すと Connected – Tokyo を引き継ぐ | ◎ |
| アプリの外（Tailscale のメニュー）で出口を外す → 1 分以内に「A server is still running」と Delete it / Use it が出る → Delete it で消える | ◎ |
| 自動切断（`VOD_TEST_AUTO_DISCONNECT_SECS=300`）: 10 分前の通知、5 分後に自動で切断して VPS を消し、通知 | ◎ |
| 終了の確認の画面: 「Delete the server and quit」で VPS を消してから終了 | ◎ |

見つけて直したこと:

- **消した VPS の Ephemeral ノードが、数分のあいだ Tailscale で「オンライン」のまま残る**（同じホスト名 `vpn-on-demand-nrt` で）。同じ国にすぐ再接続すると、古いノードを新しい VPS と取り違え、つながらない出口に切り替えるおそれがあった。`tailscale status --json` のピアの `Created`（tailnet に参加した時刻）を読み、VPS の作成時刻（Vultr の `date_created`）より前に参加したノードは無視するようにした（`MeshStatus::peer`）。単体テストを 2 件足して 25 件
- 開発用に、リポジトリ直下に `.gitignore`（`.env` を除外）と `.env.example`（`VULTR_API_KEY`）を足した。`.env` はユーザーが値を入れ、VPS が消えたことを WSL から API で確かめるのに使った（値は表示していない）

気づいたこと（Phase 6 以降で扱う）:

- 自動切断の通知の文面は設定の時間（「after 4 h」）を出すので、テスト用に秒へ縮めたときは実際の時間と合わない（テストのときだけの問題なので、そのままにする）
- 消した VPS のノードは、Ephemeral なので Tailscale が後で自動で消す。アプリは何もしなくてよい
