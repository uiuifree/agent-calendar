# Changelog

## 0.1.0 (unreleased)

- Day, week and month calendar and stats views for Claude Code and Codex CLI sessions, filtered by host and repository
- Per-session summaries via `claude -p` (English or Japanese)
- API-price cost estimates split into agent, subagents and advisor (Claude Code)
- Conversation history; send the next instruction to a session from the page
- Resume Claude Code sessions in the background with Remote Control; copy resume commands
- Schedules that run a prompt with Claude Code or Codex in a chosen directory (once, weekdays, or every N minutes)
- Repositories view: GitHub repositories of the organizations chosen in Settings, matched with local clones; start a session or clone
- Pin conversations to the sidebar; summarize a session on demand; paste screenshots into instructions; copy a schedule
- Collect sessions from another machine over HTTPS with a pinned certificate (`share` / `remote add`)
- Permission choices read-only / edit / auto; Claude's permission prompts appear on the page with allow, deny, or always allow here
- Changes tab with a GitHub-style file list and diffs; links to the repository, branch and compare page on GitHub
- Start a session in a separate git worktree on a new branch; per-repository session list in the Repositories view
- Calendar stays at `/`; other views, the open session, its tab and full screen are kept in the URL
- English and Japanese UI; single binary with the web UI embedded; `service install` for systemd
