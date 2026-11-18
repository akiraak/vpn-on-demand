# DONE

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
