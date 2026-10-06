# Changelog

## 0.1.3

- Start here: the list of branches to cut the new worktree from now shows branches on GitHub as `origin/name`; only branches that exist just on this machine keep their bare name. The default branch reads `origin/main`, so it is clear where the new branch starts from
- Permission choice when sending the next instruction now starts at auto (was edit); the last choice is remembered in the browser and shared with Start here

## 0.1.2

- Repositories view: the GitHub repository list is now reused for 24 hours (was 10 minutes); Reload still fetches it again

## 0.1.1

- The page now listens on port 23848 by default (was 8082, which collides with common development servers); `--port` still changes it
- Pages can no longer be shown inside a frame of another site (`X-Frame-Options: DENY`, `frame-ancestors 'none'`)
- Start here: choose the model; cut the new worktree from the GitHub default branch or a branch you pick, or work in a worktree that already exists; the permission choice starts at auto
- Per-repository menu to fetch from GitHub and to remove finished worktrees and local branches
- Session details: hide the details at the top to give the conversation and diffs more room; in full screen, ✕ goes back to the side panel
- Picking a day in the sidebar calendar from the Repositories or Schedules view goes to the calendar

## 0.1.0

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
- Daily update check against GitHub Releases; one-click (or automatic) update verified with sha256
- Screenshot mode that replaces names and text with sample content for sharing screenshots
- English and Japanese UI; single binary with the web UI embedded; `service install` for systemd
