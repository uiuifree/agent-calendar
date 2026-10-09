# Changelog

## 0.1.9

- Conversation: the text box stays usable while an instruction is running (here, in another tab, or in a terminal), so you can write notes or the next instruction ahead. Only Send waits until it has finished. What you wrote is kept when the run ends

## 0.1.8

- Session details: when the top is folded, the repository and branch still show next to the title if the panel is wide enough

## 0.1.7

- Changes: images (PNG, JPEG, GIF, WebP) are shown before and after side by side instead of just "binary"; click one to open it at full size. They are hidden in screenshot mode

## 0.1.6

- Mark a session as done from its Overview (Mark done / Undo done), even when the summary says it is in progress. It then shows as done on the calendar, in pins and in the repository list. The mark lasts until the session continues
- Stats: choose Day as well as Week and Month (‹ › move by the chosen unit), and see what is left to do in that period under the table: sessions in progress, or not done with something left in their summary, with what is left, a link to open the session, and Mark done. Sessions whose summary says done are not listed
- Stats: Summarize now writes a summary of the day, week or month (overview, by repository, left to do) from the summaries of its sessions, and keeps it; it shows again when you come back to that period. Only the short overview is shown at first; the details by repository open on a click
- `agent-calendar todo [--from DATE] [--to DATE]` lists the sessions of those days that are in progress, or not done with something left to do, and `agent-calendar done <id> [--undo]` marks one as done, so you can go through them from a chat with an agent

## 0.1.5

- Fix: in full screen, session details were pushed to the right and cut off when the conversation had a long line (since 0.1.4)

## 0.1.4

- Session details open in tabs at the top of the right panel: picking another session replaces the tab unless you keep it with ＋, kept tabs stay side by side, and a half-written instruction survives switching tabs
- Drag the edge between the calendar and the right panel to change its width (per Overview / Conversation / Changes; double-click to reset)
- Conversation: a Stop button stops the instruction that is running for the session (one you sent, or one started from Start here or another tab). Changes it already made are kept
- An instruction sent from the page, Start here, and a schedule may now run for up to 60 minutes (was 15) before it is stopped
- Settings: when Check now finds a newer version, the Update button in the header appears right away (it used to appear only after reloading the page)

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
