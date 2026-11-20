# ドキュメントを整備する

## 目的・背景

このアプリは誰でも使えるように公開する（2026-11-16 に決めた）。完成の条件は、GitHub の README（英語）だけを読んで、Vultr と Tailscale のアカウントを作るところから、アプリを入れて使い、片付けるところまでできること。

今の README は 2 行だけで、インストーラは手元の WSL でしか作れない。README を書き、インストーラを GitHub Actions で作って Releases で配る。

派生元: 「Windows 版を完成させる」の Phase 6 と Phase 7（[プラン](archive/windows-app.md)）

## 対応方針

### Step 1: README（英語）を書く

読む人は、Vultr も Tailscale も使ったことがない人。上から順に進めれば使える並びにする。

1. 何ができるか、しくみ（PC → Tailscale → Vultr の VPS → インターネット）、向いていない使い方
2. 用意するもの（Windows 10 / 11、Tailscale、Vultr のアカウント）と費用
3. セットアップ: Tailscale を入れる → tailnet のポリシー → auth key → Vultr の API キー → アプリを入れる（SmartScreen の越え方）→ 設定画面に入れて Test setup
4. 使い方（接続、切断、自動切断、トレイ、終了）
5. 費用の止め方（VPS が残っていないかを Vultr で確かめる方法を含む）
6. 困ったとき（アプリが出すメッセージごと）
7. 新しい版への入れ替え、アンインストールと片付け（キーの取り消し、ポリシーの行）
8. セキュリティ（キーの保存先、auth key が VPS の user data に残ること、Vultr の API キーの権限）
9. ソースからのビルド（要点だけ。詳しくは CLAUDE.md）

書く内容はアプリの画面の文言と実際の動きに合わせる（ポリシーの JSON、キーの作り方は設定画面と同じにする）。

次の 2 つは別のタスクで決めるので、README には今の動きを書く。決まったら README を直す。

- 「出口ノードから tailnet の中へは通さない設定を検討する」（grants の例）
- 「auth key の期限への対策を決める」（今は、期限が切れたら作り直して設定画面に入れ直す、と書く）

### Step 2: GitHub Actions でインストーラを作る

- `.github/workflows/release.yml`: `v*` のタグを push したとき（と手動の実行）に、`windows-latest` で単体テストを通してから NSIS のインストーラを作り、**下書き**の Release に添付する（`tauri-apps/tauri-action`）
- タグと `tauri.conf.json` の `version` が違えば失敗させる
- コード署名はしない（CLAUDE.md の決定）

### Step 3: 下書きの Release を作り、README だけを見て入れて確かめる

- `v0.1.0` のタグを push して、下書きの Release ができることを確かめる（**タグの push の前にユーザーに確認する**）
- sx360 で開発版をアンインストールし、設定とキーも消して、README の手順だけで入れ直して通しで使う（**VPS を作るので、前にユーザーに確認する**）
- つまずいたところは README を直す
- README に載せる画像（アプリの画面、SmartScreen の警告）をここで撮る

### Step 4: 公開する

- **Release の公開はユーザーに確認する**
- README のダウンロードのリンクが公開した Release を指すことを確かめる
- TODO.md から DONE.md へ移し、このプランを `archive/` へ移す

## 影響範囲

- このリポジトリ: `README.md`、`app/README.md`、`.github/workflows/release.yml`（新規）、`docs/images/`（新規。README の画像）、TODO.md、DONE.md、CLAUDE.md（リリースの手順）
- GitHub: Actions の実行、タグ、Release（下書き → 公開）
- sx360: アプリの入れ直し。Step 3 で Vultr に VPS を作り（数分で 1 セント未満）、出口ノードを切り替える

## テスト方針

- README: 書いた手順と文言を、アプリの画面（`app/index.html`、`app/src/main.ts`）と Rust 側のメッセージに突き合わせる。リンク先が開くことを確かめる
- Actions: 実行が成功し、下書きの Release にインストーラが付くこと。そのインストーラが sx360 に入り、起動すること
- 通しの確認: Step 3 のとおり、README だけを見て、まっさらな状態から接続と切断ができること。終わったら VPS が残っていないことを確かめる

## 進み具合

- 2026-11-19: プランを作った
- 2026-11-19: Step 1 と Step 2。README を書き、`.github/workflows/release.yml` を書いた。README の内容はアプリの文言、テスト名（接続に失敗したら VPS を消す、など）、Vultr の公開 API（33 都市、20 か国、最小プランは $0.007/時）に突き合わせた。ワークフローはまだ動かしていない（Step 3 でタグを push して確かめる）
- 2026-11-20: Step 3 の前半。`v0.1.0` のタグを push した。1 回目の Actions は単体テスト `script_is_valid_bash` で失敗した（`windows-latest` の `bash` は WSL の起動役で、ディストリビューションが無くて失敗する）。テストを Unix だけにし、タグを付け直して 2 回目で成功した（約 11 分）。下書きの Release に `VPN.on.Demand_0.1.0_x64-setup.exe`（2.2 MB）が付いた。README に書いたファイル名と同じ。残りは sx360 での入れ直しと通しの確認、画像
- 2026-11-20: Step 3 の後半。sx360 で開発版をアンインストールし、設定のフォルダと資格情報マネージャーのキー 2 つを消してから、ユーザーが下書きの Release のインストーラで入れ直した。キーを入れて東京に接続し、U-NEXT と TVer が見られた。切断のあと、Vultr に VPS が無いことと、出口ノードが外れたことを確かめた
  - アンインストーラは `%LOCALAPPDATA%\io.github.akiraak.vpn-on-demand`（画面のキャッシュ）を消さないと分かったので、README の片付けの手順に足した
  - 画像: Claude Code から PowerShell でアプリのウィンドウだけを撮った（`PrintWindow`。ボタンは WebView のウィンドウにクリックのメッセージを送って押すので、マウスは動かない）。未接続、接続中、Settings の 3 枚を `docs/images/` に置いて README に載せた。接続中の画面のために東京にもう 1 回接続し、切断して VPS が消えたことを確かめた。SmartScreen の警告は撮っていない（文章だけ）
