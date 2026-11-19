# 接続までの時間を速くする

## 目的・背景

- 今は「Connect」を押してから Connected まで 3〜5 分かかる（2026-11-17 に sx360 で確かめた。[windows-app](archive/windows-app.md) の Phase 5）。NordVPN は数秒でつながっていたので、使うたびの待ちが長い
- 試作（jp-exit）では「VPS を作ってから Tailscale に参加するまで約 2 分」だった。残りの時間がどこで使われているかは測っていない
- 目標: まずどこに何秒かかっているかを測る。そのうえで、今の方式の中での改善と、方式そのものの見直し（VPS の事業者、Tailscale を使わない経路など）を比べて、ユーザーと決める
- 変えない方針（CLAUDE.md の「設計の方針」）: 課金を止めることを最優先にする。使わない月は（ほぼ）0 円。専用の VPN クライアントやドライバを入れない。誰でも README を見て使える

## 今の接続の流れ（`vpn.rs` の `connect`）

| 段階 | 中身 | 時間 |
|---|---|---|
| 1. 確認 | `tailscale status`、Vultr でラベルの VPS を探す | 未計測（数秒の見込み） |
| 2. 作成 | Vultr のプラン、空き、OS の ID を読み、VPS を作る API を呼ぶ | 未計測（数秒の見込み） |
| 3. VPS の起動 | Vultr が VM を用意して Ubuntu 24.04 が起動する | 未計測 |
| 4. cloud-init | IP 転送、`curl https://tailscale.com/install.sh \| sh`（apt のリポジトリを足して `apt update` と `apt install`）、`tailscale up` | 未計測 |
| 5. 参加を待つ | 5 秒ごとに `tailscale status` を読み、新しいピアがオンラインになるのを待つ | 3+4 の終わりまで |
| 6. 承認を待つ | `autoApprovers` があればすぐ `ExitNodeOption` になる見込み | 未計測 |
| 7. 切り替え | `tailscale set --exit-node` | 未計測 |
| 8. IP の確認 | ipinfo.io で外の IP と国を見る（失敗したら 3 秒後に再試行） | 未計測 |

### 「サーバーの起動までが長すぎるのか」への今の見立て

- 試作の約 2 分は 3 と 4（VM の起動と Tailscale のインストール）を合わせた時間で、3〜5 分のうちの大半はここだと見ている。ただし 3 と 4 の内訳はまだ分からない
- 4 は、起動直後の Ubuntu で `apt update` をし、Tailscale を入れる。起動直後は Ubuntu の自動更新（unattended-upgrades）が apt のロックを取って、待たされることがある。この待ちが長い可能性がある
- 5〜8 はアプリ側の待ち方で、5 秒ごとの確認による遅れは最大で数秒しかない。ここを詰めても大きくは変わらない見込み
- まず Phase 1 で測ってから判断する

## 対応方針

### Phase 1: どこに何秒かかっているかを測る

- アプリに、段階ごとの時刻を記録する仕組みを足す（ログファイル。キーや IP アドレスの全文は書かない）。記録する時刻:
  - Connect を押した時刻、作成の API が返った時刻
  - Vultr の `server_status` / `power_status` が「起動した」になった時刻（Vultr の API を読んで見る）
  - Tailscale のピアが現れた時刻と、ピアの `Created`（tailnet に参加した時刻）
  - `ExitNodeOption` になった時刻、切り替えた時刻、IP を確かめた時刻
- VM の中の内訳（起動、cloud-init の各コマンド、apt のロックの待ち）は、cloud-init の各段階で時刻をファイルに書き、参加したあとに Tailscale SSH で `cloud-init analyze show` とそのファイルを読む。Tailscale SSH が tailnet のポリシーで許されていなければ、ポリシーの変更をユーザーに提案する（**ポリシーの変更の提案は先に確認する**）
- 東京（nrt）で 3 回測る。ほかの国も 1 回（例: ロサンゼルス）測り、国で差があるかを見る
- **VPS を作るので、測る前にユーザーに確認する**（1 回で約 $0.01、合わせて $0.05 ほど）。毎回、終わったら VPS が消えたことを確かめる。出口ノードの切り替えも起きるので、そのことも合わせて確認する

#### 測り方（2026-11-18 に決めた）

アプリに記録を足して Windows で手で押す代わりに、**計測用のプログラム `app/src-tauri/examples/measure.rs`** を作り、WSL から動かす。Phase 2 で 1 つ変えるたびに測り直すので、何度でも同じ条件で測れるようにする。

- アプリと同じ `Vpn::connect` / `disconnect` を、本物の Vultr と Tailscale（WSL から Windows の `tailscale.exe`）で動かす。待ち方（5 秒ごとの確認など）もアプリと同じ
- 並行して 2 秒ごとに Vultr の `GET /instances/{id}`（`status`、`power_status`、`server_status`）と `tailscale status` のピアを読み、変わった時刻を記録する。ピアの `Created` も記録する
- VPS の中の内訳: 計測のときだけ、user data に時刻を書く行を足す（`/var/lib/vod-measure/marks` に、スクリプトの開始、カーネルの起動時刻、Tailscale のインストールの前後、`tailscale up` の前後、終わり。`set -x` の各行にも時刻を付ける）。cloud-init が終わったら `cloud-init analyze`、`systemd-analyze`、apt の履歴、apt-daily / unattended-upgrades のジャーナル、cloud-init の出力を集め、VPS の Tailscale の IP の 8099 番で配る。プログラムはこれを読んで記録に混ぜる
  - Tailscale SSH を使わないので、tailnet のポリシーの SSH の規則を変えなくてよい。PC から `tag:vpn-on-demand` の 8099 番へ届くこと（今のポリシーが全許可なら届く）だけが要る
  - `set -x` の出力には auth key が出るので、VPS の上とプログラムの両方で `tskey-...` を伏せてから保存する
- 終わったら必ず切断し、Vultr にアプリのタグの VPS が残っていないことを確かめる。Ctrl-C でも切断して消す。始める前にアプリのタグの VPS があれば、本物の接続を邪魔しないように止める
- キーは `.env` の `VULTR_API_KEY` と `TAILSCALE_AUTH_KEY`（変数名は `.env.example`）。結果は `app/src-tauri/measure-out/<region>-<時刻>/`（Git 管理外）に `timeline.txt`、`events.json`、VPS の中のファイルを保存し、数字はこのプランの「結果」に書く
- 実行: `cd app/src-tauri && set -a && . ../../.env && set +a && cargo run --example measure -- nrt JP`（`--print-user-data` で、何も作らずに user data を表示する）

#### 結果（2026-11-18、vc2-1c-1gb、Ubuntu 24.04、sx360 の WSL から）

**接続は 100〜125 秒（東京の中央値 110 秒）。切断は約 36 秒。4 回とも VPS は残っていない。** 段階ごとの秒数（左の 3 列が東京 nrt、右端がロサンゼルス lax）:

| 段階 | nrt 1 | nrt 2 | nrt 3 | lax |
|---|---|---|---|---|
| 確認と作成の API（プラン、空き、OS の ID、`POST /instances`） | 4.4 | 2.9 | 3.7 | 6.2 |
| **Vultr が VM を用意して起動する**（作成の API が返ってから、カーネルが起動するまで） | 47.5 | 60.6 | 48.6 | 45.2 |
| **起動からスクリプトの開始まで**（カーネル約 5 秒、systemd、cloud-init の init-local / init-network / modules-config） | 24.4 | 26.8 | 22.2 | 20.8 |
| **Tailscale のインストール**（`install.sh`。`apt-get update` 約 14 秒、`apt-get install` 約 6 秒） | 24.1 | 26.8 | 25.2 | 24.8 |
| `tailscale up` から、オンラインで出口ノードとして承認されるまで | 2.2 | 2.8 | 2.3 | 2.5 |
| アプリが気づくまで（5 秒ごとの確認） | 0.7 | 2.8 | 0.5 | 0.1 |
| 出口ノードの切り替え（約 0.5 秒）と IP の確認（ipinfo.io。切り替え直後に失敗して 3 秒後に再試行することがある） | 6.5 | 2.4 | 6.4 | 1.1 |
| **合計（Connect から Connected）** | **109.8** | **125.1** | **109.0** | **100.7** |

分かったこと:

- **VM の用意（約 48 秒）が最大**で、アプリからは変えられない。Vultr の状態は、作成の数秒後に電源が `stopped` / `locked`（イメージを書いている）になり、約 40 秒後に `running` / `ok`。カーネルが起動するのはさらに約 10 秒後（`ok` になっても、まだ起動していない）
- **起動からスクリプトの開始まで約 24 秒**。cloud-init の中だけで約 13 秒（init-local で Vultr のメタデータを読むのに約 3 秒、init-network、modules-config）。残りはカーネルと systemd
- **Tailscale のインストールが約 25 秒**。うち `apt-get update` が約 14 秒（12 MB を取り、パッケージの一覧を作る）。apt のロックの待ち（unattended-upgrades）は 4 回とも起きていない。静的なバイナリにすれば数秒になる見込み
- `autoApprovers` が効いていて、承認の待ちは無い（参加から 1〜2 秒で `ExitNodeOption`）
- アプリ側の待ち（作成の API、5 秒ごとの確認、IP の確認）は合わせて約 10 秒
- 国による差は小さい（lax は 101 秒）
- **2026-11-17 の「3〜5 分」より大幅に短い。** 計測のプログラムはアプリと同じ処理なので、前回は承認の待ち（`autoApprovers` を入れる前）や、名前の同じ古いピアの扱い（その後に直した）で遅れていた可能性が高い。Phase 4 で、アプリの画面からも 1 回測って確かめる
- **切断の約 36 秒は、Vultr の API の待ち。** `tailscale set --exit-node=` は 0.4 秒で終わるが、直後の `list_servers` が約 31 秒かかって失敗し、4 秒後の再試行で成功している（4 回とも同じ）。経路が変わった直後の TCP の接続が応答なしで止まり、OS の接続のタイムアウト（SYN の再送）まで待っていると見ている。HTTP クライアントに短い接続のタイムアウト（例: 5 秒）を付ければ、数秒になる見込み。課金を止める処理なので、Phase 2 で直す
- 計測の不具合: 1 回目は `PS4` を `export` したため、Tailscale の `install.sh`（dash）が `PS4` を読めずにエラーを出した（インストールは成功）。2 回目から `export` しない

今の方式で詰められそうなのは、Tailscale のインストール（約 20 秒）、アプリ側の待ち（約 5 秒）、起動の一部（OS イメージ次第）。VM の用意の約 48 秒は Vultr で VPS を作る限り残るので、今の方式の下限は 1 分強と見ている。数秒〜数十秒にするには、方式の見直し（Phase 3）が要る

### Phase 2: 今の方式の中での改善を試す

Phase 1 の結果で、時間を使っている段階から順に試す。1 つ変えるたびに測り直す。

- **Tailscale のインストールを速くする**
  - `install.sh`（apt）をやめて、Tailscale の静的なバイナリ（tgz）を落として置き、`tailscaled` を起動する。`apt update` と apt のロックの待ちが無くなる見込み
  - または、cloud-init の初めに unattended-upgrades を止める
- **Tailscale を入れたスナップショットから VPS を作る**
  - cloud-init で入れる時間が無くなる。ただし Vultr のスナップショットは保存している間ずっと料金がかかる（金額は Phase 3 で調べる）ので「使わない月は 0 円」から外れる。スナップショットの作り方と更新（Tailscale の版が古くなる）、誰が作るか（アプリが自動で作るか）も決める必要がある
- **OS とプランを替える**
  - Debian 12 など、起動が速い OS イメージがあるかを測る
  - vc2 より上のプラン（高周波数など）で起動が速くなるか。値段が上がるので、差が大きいときだけにする
- **アプリ側の待ち方**
  - 確認の間隔を 5 秒から 2 秒にする、VPS を作る前の API の呼び出し（プラン、空き、OS の ID）をまとめるか前もって読んでおくなど。小さな改善なので、ほかとまとめて入れる

### Phase 3: 方式そのものの見直しを机上で調べる

Phase 2 と並べて比べる。調べるだけで、アカウントは作らない。

- **ほかの事業者**: 日本（東京か大阪）にリージョンがあり、時間か秒の課金で、API で VM を作れるもの。例: Akamai（Linode）、AWS（EC2、Lightsail）、Google Cloud、Fly.io など。比べる点:
  - VM の起動の速さ（公式の数字や測った記事）
  - **止めておけるか**: 消さずに止めて、止めている間はディスク代だけになる事業者なら、2 回目からは「起動するだけ」になり、数十秒になる見込み（Vultr は止めても課金されるのでできない）
  - 使わない月の料金（ディスク、スナップショット、IP アドレス）
  - API キーの権限を絞れるか（Vultr のキーはアカウント全体を操作できる）
  - データセンターの IP で動画サイトに止められないか（2026-11-16 の時点では Vultr の東京は 3 サイトとも見られた）
  - 利用者の準備の手間（アカウント、支払い、キーの作り方）。誰でも使えるようにする方針に合うか
- **Tailscale を使わない経路**: たとえば WireGuard を直接使う。Tailscale の参加と承認の待ちは無くなるが、PC に WireGuard のクライアントが要るので、「専用の VPN クライアントを入れない」方針と合わない。調べたうえで、方針を変える価値があるかをユーザーに聞く
- **常に動かしておく**: いちばん速い（数秒）が、月額がかかる（Vultr の最小で月 $5 ほど）。比べる基準として載せる

### Phase 4: 決めて作る

- Phase 1〜3 の結果を表にまとめ、ユーザーと方式を決める
- 決めた方式を実装し、Phase 1 と同じやり方で測って、速くなったことを確かめる
- 接続中の画面（タイムライン）の段階と「usually 3–5 minutes」などの文言を、新しい時間に合わせて直す

## 影響範囲

- `app/src-tauri/src/vpn.rs`（流れと待ち方）、`cloud_init.rs`（user data）、`vultr.rs`（状態の取得、OS とプラン）、`lib.rs`（計測のログ）
- 方式を替えるなら、事業者のクライアント、設定画面のキー、README、CLAUDE.md の「設計の方針」も変わる
- 画面の文言（`main.ts` のタイムラインと所要時間）

## テスト方針

- 単体テスト: user data の中身、待ち方の変更は今の偽物（Vultr と Tailscale のトレイト）を使ったテストで確かめる。`cargo test`、`cargo clippy`、`npm run build`
- 実測: sx360 で本物の VPS を作って測る（**毎回ユーザーに確認する**）。同じ条件で 3 回ずつ測り、中央値で比べる。測り終えたら VPS が残っていないことを Vultr の API で確かめる
- 速くしても、切断で VPS が消えること、アプリが落ちたあとに残った VPS を見つけて消せることは変わらないことを確かめる
