# agent-calendar

[English](README.md) | [日本語](README.ja.md)

**AI コーディングエージェントのセッションを、手元のカレンダーで見るツール。** [Claude Code](https://docs.claude.com/en/docs/claude-code)
と [Codex CLI](https://github.com/openai/codex) がホームディレクトリに残しているセッションの記録を読み、
日・週・月のカレンダーで見せます。セッションごとの要約、案件ごとの作業時間と API 単価で換算した費用、会話の履歴、
セッションの続きの指示もできます。決めた時刻にリポジトリでエージェントへ指示を実行させる予定も組めます。

すべて自分のマシンの中で動きます。画面は `127.0.0.1` でだけ出します。

## できること

- **カレンダー**: 日・週・月の表示を Google カレンダー風に。ホストとリポジトリで絞り込める
- **セッションの詳細**: 要約（やったこと、完了／途中／相談のみ、残っていること）、本体・サブエージェント・advisor 別の費用、
  セッション中に自分が入れた git のコミット、会話の履歴。「今すぐ要約」で好きなときに要約できる
- **集計**: 案件ごとの作業時間・セッション数・費用を日・週・月で。リポジトリと案件の対応は画面で選ぶ
- **続き**: Claude Code・Codex のセッションに画面から続きの指示を出す（読むだけ／ファイルの編集まで／自動判定を選ぶ）。
  Claude が許可の要るコマンドを使うときは、画面に許可・拒否の確認が出る。
  Claude Code のセッションは Remote Control で裏に再開もできる。`claude --resume` / `codex resume` のコマンドもコピーできる
- **予定**: 決めたディレクトリで Claude Code か Codex に指示を実行させる。1 回だけ・曜日指定・時間帯の中で N 分ごと、
  別の git worktree で動かすこともできる。結果はカレンダーに出る
- **リポジトリ**: 組織を選ぶと、その組織の GitHub のリポジトリと手元の clone（設定したフォルダの直下を `origin` で突き合わせ）、
  最後に作業した日、GitHub の最終更新日が並ぶ。扱う組織は設定で選んだものだけ。手元にあればそこで Claude Code か Codex の
  新しいセッションを始められ、無ければ clone できる。`gh` のログインを使う
- **ピン留め**: 進めている会話をピン留めすると、サイドバーの上に並ぶ
- **スクリーンショット**: 指示に画像を貼り付け・ドロップで付けられる（PNG・JPEG・GIF・WebP、5 枚まで、1 枚 5MB まで）
- **別のマシン**: `share` / `remote add` で、別のマシンのセッションを HTTPS（証明書の固定と合言葉）で取り込む
- 画面は英語と日本語

## インストール

Linux（WSL2 を含む）と macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/uiuifree/agent-calendar/main/install.sh | sh
```

`~/.local/bin` に `agent-calendar` が入ります。systemd のある Linux なら続けて:

```sh
agent-calendar service install     # systemd のユーザーサービスとして常駐させる
```

それ以外は:

```sh
agent-calendar serve
```

を動かして <http://127.0.0.1:8082/> を開きます。

### ソースから

```sh
git clone https://github.com/uiuifree/agent-calendar && cd agent-calendar
(cd web && npm ci && npm run build)   # 画面はバイナリに埋め込む
cargo install --path .
```

## 使い方

```text
agent-calendar serve [--port 8082] [--no-summarize] [--summary-model sonnet] [--summary-lang en|ja]
agent-calendar scan
agent-calendar summarize [--limit 20] [--model sonnet] [--lang en|ja]
agent-calendar service install [--port 8082] [--no-summarize] [--summary-lang en|ja]
```

`serve` は 5 分ごとに記録を読み直し、止まったセッションを 1 時間ごとに 1 回 10 件まで要約します（7〜22 時）。
時間帯と間隔は画面の設定で変えられます。
要約の言語は、`$LANG` が `ja` で始まれば日本語、そうでなければ英語です。

## しくみ

| 記録 | 読む場所 | 補足 |
|---|---|---|
| Claude Code | `~/.claude/projects/*/*.jsonl` | サブエージェントの記録（`<session>/subagents/`）は親のトークン・費用に足す |
| Codex CLI | `~/.codex/sessions/**/rollout-*.jsonl` | トークン数のみ（金額は出さない）。サブエージェントのスレッドは数えない |

- 集計は `~/.local/share/agent-calendar/agent-calendar.db`（SQLite）に置きます。Claude Code は古い記録を消します（設定の
  `cleanupPeriodDays`）が、一度集計したセッションはカレンダーに残ります
- 作業ディレクトリは git のリポジトリ単位にまとめます（worktree は本体のリポジトリに寄せる）
- **要約**は、自分の Claude Code のログインで `claude -p --no-session-persistence` を呼んで作ります。Claude のプランの使用量を使います。
  送るのは依頼とエージェントの文章だけで、ツールの入出力は送りません。`--no-summarize` で止められます
- **費用**は Anthropic API の単価（`src/pricing.rs`、2026-09-25 時点）で換算した目安で、サブスクの請求額ではありません。
  advisor とサブエージェントの分を含みます。単価の分からないモデルを含むときは `+` を付けます
- **作業時間**は、セッションをまたいで 10 分の枠を合わせて数え、30 分以内の中断はつなげます。エージェントが動いていた時間で、
  実際の作業時間とは一致しません
- **再開**は、一度裏に出したセッションには `claude respawn`、それ以外は `claude --bg --resume <id> --remote-control` を使います。
  Linux では `systemd-run --user --scope` の中で起動するので、サービスを再起動しても再開したセッションは止まりません

## セキュリティとプライバシー

- サーバーは `127.0.0.1` で待ち受け、`Host` が `127.0.0.1|localhost|[::1]:<port>` のものだけ受けます（DNS rebinding を防ぐ）。
  書き込みと再開は JSON の本文を必須にし、別のオリジンからのリクエストは断ります
- 記録には打った内容がそのまま入っています。マシンの外に出るのは、`claude -p` を通した要約の依頼だけです

## 対応状況

agent-calendar は、安定した API として公開されていない記録の形と CLI のコマンド（`~/.claude/projects`、`claude agents --json`、
`claude respawn`、`~/.codex/sessions`）に依存しています。**Claude Code 2.1.288** と **Codex CLI 0.154.0** の Linux（WSL2）で
確かめています。macOS 版はまだ確かめていません。新しい版で動かなくなったら、版番号を添えて issue を立ててください。

## よくある質問

### 先週 Claude Code で何をしたか見るには？

週表示を開き、矢印（または `k`）で 1 週戻ります。セッションごとに、止まったあとで書いた要約が付いています。

### Claude Code や Codex の作業時間を案件ごとに出すには？

集計を開き、リポジトリごとに案件を一度選べば、週・月で切り替えて見られます。

### Claude Code の使用量を API の単価に直すといくら？

集計と各セッションの詳細に、本体・サブエージェント・advisor 別の API 単価換算を出しています。

### スマホから Claude Code のセッションを再開できる？

できます。セッションの「Remote Control で再開」を押すと、手元のマシンの裏で起動し、Claude アプリの Code の一覧に出ます。
ボタン自体は手元の画面にあるので、押すのはそのマシンからです。

### 記録をどこかに送っている？

要約のときだけ、自分の `claude -p` を通して送ります。`--no-summarize` にすれば何も出しません。

### Windows で使える？

Claude Code を WSL2 で使っているなら、WSL2 の中で使えます。Windows ネイティブには対応していません。

## まだ対応していないこと

- Codex（OpenAI）のモデルの金額、Codex のサブエージェントのスレッド、Codex のセッションの Remote Control での再開
- ダークモード

## ライセンス

次のどちらかを選べます。

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)、<https://www.apache.org/licenses/LICENSE-2.0>）
- MIT license（[LICENSE-MIT](LICENSE-MIT)、<https://opensource.org/licenses/MIT>）

### コントリビューション

特に断りがない限り、このプロジェクトに取り込まれることを意図して提出されたコントリビューションは、
Apache-2.0 の定義に従い、追加の条件なしに上記のデュアルライセンスで提供されるものとします。

---

agent-calendar は Anthropic・OpenAI・Google とは関係がなく、承認も受けていません。Claude と Claude Code は Anthropic, PBC の、
Codex は OpenAI の、Google カレンダーは Google LLC の商標です。
