# DONE

## 2026-11-18

- [x] デザインを改善する（アプリのアイコン、画面の見た目、初回の案内） [plan](docs/plans/archive/design.md)
  - 3 案からユーザーが C（経路を見せる）を選び、アイコン、Connect（経路の絵、タイムライン、接続中のタイル）、Settings（4 つの手順のチェックリスト）、終了の確認を作り直した
  - Phase 5 で、出口の場所を 1 行にまとめ、Change で一覧を開くようにした（4 案から A）。窓は 560 × 540
  - sx360 で画面、窓のアイコン、Test setup を確かめた。トレイなどのアイコンはユーザーが見て終了とした。VPS を作る接続の画面は偽物の状態でだけ確かめた（接続の処理は変えていない）
- [x] [Windows] コード署名をするか決める
  - しないと決めた。SmartScreen の警告の越え方を README で案内する（CLAUDE.md の「設計の方針」）
- [x] 開発環境を作る（sx360 の WSL）
  - Windows 版は WSL からクロスビルドする。入れたものと手順は CLAUDE.md の「開発環境」。m1 の分は「[macOS] m1 で確かめる」に含める

## 2026-11-17

- [x] Windows 版を完成させる（Phase 1〜5） [plan](docs/plans/archive/windows-app.md)
  - 試作を `app/` に移し、接続・切断（Vultr、cloud-init、Tailscale の CLI）、画面（英語。Connect / Settings）、トレイ、消し忘れの対策（自動切断、起動時の検出、終了時の確認、外で出口を外したときの検出、二重起動の防止）を作った。単体テスト 25 件
  - sx360 で本物の VPS を使って確かめた。消した VPS のノードと取り違える不具合を見つけて直した
  - 残りの Phase 6（README、初回の案内）と Phase 7（Releases での配布）は、TODO の「デザインを改善する」「ドキュメントを整備する」に分けた
  - このタスクで一緒に済んだもの:
    - コア処理: Vultr の API の処理、cloud-init の user data、接続の流れ、切断の流れ、偽物を使ったテスト
    - 画面: 英語にする、メイン画面、設定画面（キー、Test setup、案内）、タスクトレイ
    - 消し忘れの対策: 自動切断、起動時の残った VPS の検出、終了時の警告
    - Tailscale: 出口ノードの自動承認（タグ `tag:vpn-on-demand` と `autoApprovers`。設定画面で案内する）
    - 配布: Windows のインストーラ（NSIS、ユーザー単位、署名なし）を作れるようにした
