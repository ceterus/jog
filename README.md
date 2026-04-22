# jog

Jog your memory before standup.

A small Rust CLI that pulls your Jira activity from the previous work day (plus
today so far) and prints a standup-ready summary: status transitions you made,
comments you wrote, fields you touched, what's in progress now, and sprint
health.

Previous work day = yesterday, or Friday if today is Monday (Sunday also rolls
back to Friday).

## Install

```bash
cargo build --release
cp target/release/jog /usr/local/bin/   # or anywhere on PATH
```

## Setup

Credentials live in your OS's native credential store (macOS Keychain,
Linux Secret Service, or Windows Credential Manager) via the `keyring` crate.
One-time prompt:

```bash
jog setup
```

You'll be asked for:

- **Jira base URL** (e.g. `https://your-org.atlassian.net`)
- **Jira email** (the Atlassian account email)
- **API token** — create at
  https://id.atlassian.com/manage-profile/security/api-tokens
  (use "Create API token", not the scoped variant)

Verify:

```bash
jog config
```

Shows the stored base URL, email, masked token, and the config file path.

### Env-var override (optional)

For CI or scripted use, env vars take precedence over the credential store:

- `JIRA_BASE_URL`
- `JIRA_EMAIL`
- `JIRA_API_TOKEN`
- `JIRA_ACCOUNT_ID` / `JIRA_DISPLAY_NAME` — fallback identity when `/myself`
  fails (rare; usually only needed on locked-down token scopes)

Leave them unset to use the credential store.

## Config file

`~/.config/jog/config.toml` (created on demand — optional). Sensible defaults
are used when fields are missing.

```toml
[jira]
base_url = "https://your-org.atlassian.net"   # used only if Keychain + env are empty
projects = ["PROJ", "INFRA"]                  # scope queries to these projects
board_id = 123                                # reserved for future use
mode     = "auto"                             # "auto" | "scrum" | "kanban"

[fields]
story_points = "customfield_10047"            # your instance's Story Points field id
sprint       = "customfield_10010"            # your instance's Sprint field id

[statuses]
in_progress = ["In Progress"]
in_review   = ["IN REVIEW"]
qa          = ["QA"]
done        = ["Done", "Closed", "Resolved"]

[ai]
provider = "none"                             # reserved

[output]
format = "text"                               # text | json | markdown
stats  = "full"                               # "full" | "summary" | "off"
layout = "card"                               # "card" | "stacked" | "plain"
```

Find your custom field IDs at
`https://<your-org>.atlassian.net/rest/api/3/field` (look for names like
"Story Points" and "Sprint").

## Run

```bash
jog                         # previous work day + today so far
jog --date 2026-04-10       # override the start date
jog --format markdown       # text (default) | markdown | json
jog --stats summary         # hide personal metrics, keep structural facts
jog --no-stats              # hide the stats panel entirely
jog --no-pr                 # skip the Bitbucket PR summary for this run
jog --stacked               # force the stacked card layout
jog --plain                 # single-column plain text (no box-drawing)
jog --debug                 # print JQL, window, issue counts, config path
```

### Bitbucket PR summary (optional)

If you use Bitbucket Cloud for your team's code, `jog setup` will prompt for:

- **Bitbucket workspace slug** — the `your-org` in `bitbucket.org/your-org/`.
- **Bitbucket API token** — leave blank to reuse your main Atlassian token
  (works if your main token has `read:account`, `read:pullrequest:bitbucket`,
  and `read:repository:bitbucket` scopes). Otherwise, create a
  Bitbucket-scoped token at
  https://id.atlassian.com/manage-profile/security/api-tokens with those
  scopes and paste it.
- **Bitbucket project keys** (optional) — comma-separated list of BB project
  keys (e.g. `CRM,INFRA`). Restricts the repo fan-out to those projects
  only. Essential in large workspaces — without it, every run iterates
  every repo you can see. Blank = scan all.

> **Why might I need a separate token?** Bitbucket Cloud uses the same
> Basic auth shape as Jira (`email:token`), but the token needs Bitbucket
> scopes. If your main jog token was created unscoped it likely Just Works;
> if it was scoped for Jira only, create a second token.

Leave the workspace blank during setup to skip Bitbucket entirely. Pass
`--no-pr` on a single run to suppress the section.

The PR summary shows three sub-sections, each hidden if empty:

- **Opened** — PRs you authored in the standup window.
- **Merged / declined** — PRs you authored that reached a terminal state
  in the window.
- **Awaiting approval** — PRs you authored that existed before the window
  (older) but were nudged during the window (comment, push, etc.) and
  still have no approvals. Stale PRs with no activity in the window
  won't surface here — a known limit of Bitbucket's BBQL (its
  OR-within-AND filters silently match nothing, so a single-conjunction
  date filter is used instead).

### Stats visibility

Some teams prefer not to surface personal performance metrics (velocity,
throughput, cycle-time averages) in standup channels. Control what shows
via `[output].stats` in config, or override per-run with `--stats` /
`--no-stats`:

- **`full`** (default) — everything: points, velocity, needed pace,
  throughput, cycle-time averages.
- **`summary`** — structural facts only: sprint name, days remaining,
  issue counts. No points, no velocity, no cycle times.
- **`off`** — stats panel hidden. Only the activity log and "Today"
  panel render.

The activity log ("Since yesterday") and "Today" panel are never
affected — those are factual history, not performance metrics.

### Text layout

The default `--format text` renderer is a "briefing card" — boxed
header, coloured icons, progress bars, and a section-per-panel layout.
It targets ~120 columns and adapts automatically:

- **≥ 80 cols** — landscape: activity / PRs / sprint sit side-by-side.
  Hidden panels (`--no-pr`, `--no-stats`) widen the remaining columns.
- **< 80 cols** — stacked: same card styling, same colour and icons,
  but sections flow top-to-bottom so nothing gets cramped.

If you prefer the stacked look on a wide terminal, force it with
`--stacked` (or set `[output].layout = "stacked"`). Stacked card is
capped at 100 columns even on very wide monitors to keep the text
readable.
- `NO_COLOR=1`, non-TTY, or `TERM=dumb` → drops ANSI colour
- `JOG_ASCII=1`, or a `C`/`POSIX` locale → uses ASCII icons (`*`, `o`,
  `v`, `^`, `?`) and `#`/`-` bars
- `JOG_WIDTH=N` → override the detected terminal width (handy for
  testing or for piping through `less -R`)

If you'd rather skip the card entirely — for piping into other tools,
pasting into ticket systems, or just a quieter terminal — use
`--plain` (or set `[output].layout = "plain"`). That renders the
legacy single-column text layout regardless of terminal width. JSON
and markdown outputs are unaffected by this setting.

## What it pulls

Activity window = `[start_date 00:00 local, now]`.

- Issues **updated** in the window where you were assignee, reporter,
  worklog author, or the person who changed status (from the changelog).
- **Status transitions** you made (from each issue's changelog, filtered to
  your accountId + the window).
- **Comments** you wrote (author + created filtered).
- **Field updates** you made (any non-status changelog item).
- **Today** — your open issues in the active sprint, grouped by status.
- **Sprint stats** — points/issues done, velocity (pts/day), required pace to
  finish on time, average cycle times (Created → Done, To Do → Done,
  In Progress, In Review, QA).

### Scrum vs Kanban

`[jira].mode` controls how the tool interprets your work:

- **`auto`** (default) — try the active sprint first, fall back to the most
  recently closed sprint (within 2 days), otherwise render a Kanban panel.
  Right for most users.
- **`scrum`** — always use sprint queries. Kanban users on `scrum` will see
  `No active sprint found.`
- **`kanban`** — skip sprint queries entirely. Renders WIP by status,
  throughput (issues/day over the last 14 days), and cycle-time averages.

### Sprint-boundary behavior

Activity queries aren't sprint-scoped, so the morning after a sprint closes
you still see yesterday's work (even if those issues moved out of the active
sprint). Sprint stats fall back to the most recently closed sprint when no
sprint is currently open, and render as `closed — ended N days ago` instead
of pretending nothing exists.

## Sample output

### Landscape card (default, ≥ 80 cols)

```
╭─ Standup · Anthony Norfleet ────────────────────────────────────────────────────────────────── Sprint 42 · 3d left ─╮
│ Since Tue Apr 21 · 2026-04-21 → 2026-04-22 09:04                                                                    │
╰─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯

 ▸ YESTERDAY · 4 tickets                     │  ▸ PULL REQUESTS                │  ▸ Sprint 42 · 3 / 14 days
 ─────────────────────────────────────────── │  ────────────────────────────── │  ─────────────────────────────────────
   ✓  PROJ-388  OAuth refresh race condition │    ↑  !234  payments            │    Issues     7 / 11   64%
     Done                                    │      Retry logic for webhook    │    ██████████████░░░░░░░░
                                             │      failures                   │
   ✓  PROJ-389  Idempotency keys on /charge  │      opened · 0 reviews         │    Points     18 / 28   64%
     Done                                    │                                 │    ██████████████░░░░░░░░
     → In Review → Done                      │    ✓  !228  payments            │
                                             │      Idempotency keys on        │    Velocity   1.6 pt/d
   ●  PROJ-401  Dashboard latency spike      │      /charge                    │    Need       3.3 pt/d
     In Progress                             │      merged                     │
     ⊕ description                           │                                 │    Cycle (avg, done)
                                             │    ⧖  !231  dashboards          │      In Progress     8h
   ●  PROJ-412  Refund webhook retry logic   │      Latency dashboard v2       │      In Review       3h
     In Review                               │      awaiting · 0 approvals     │      QA              1h
     → In Progress → In Review               │                                 │
     ⊕ sprint, story_points                  │                                 │
     ✎ "spec covers 409 but not 425 — added… │                                 │

 ▸ TODAY · 2 tickets                         │                                 │
 ─────────────────────────────────────────── │                                 │
   ●  PROJ-420  Backfill legacy accounts     │                                 │
     In Progress                             │                                 │
   ○  PROJ-425  Investigate 5xx in eu-west-1 │                                 │
     To Do                                   │                                 │

───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
  ● in-flight   ○ todo   ✓ done      ↑ opened  ✓ merged  ⧖ waiting                                           jog v0.1.0
```

In a real terminal, ticket keys (`PROJ-412`, `!234`) and progress bars
render in cyan, `✓`/merged in green, `⧖`/behind-pace in yellow, and
secondary metadata (dates, status, transitions) in dim grey.

### Stacked card (narrow terminals, or `jog --stacked`)

```
╭─ Standup · Anthony Norfleet ───────────────── Sprint 42 · 3d left ─╮
│ Since Tue Apr 21 · 2026-04-21 → 2026-04-22 09:04                   │
╰────────────────────────────────────────────────────────────────────╯

 ▸ YESTERDAY · 4 tickets
 ────────────────────────────────────────────────────────────────────
   ✓  PROJ-388  OAuth refresh race condition
     Done

   ✓  PROJ-389  Idempotency keys on /charge
     Done
     → In Review → Done

   ●  PROJ-401  Dashboard latency spike
     In Progress
     ⊕ description

   ●  PROJ-412  Refund webhook retry logic
     In Review
     → In Progress → In Review
     ⊕ sprint, story_points
     ✎ "spec covers 409 but not 425 — added test"

 ▸ TODAY · 2 tickets
 ────────────────────────────────────────────────────────────────────
   ●  PROJ-420  Backfill legacy accounts
     In Progress
   ○  PROJ-425  Investigate 5xx in eu-west-1
     To Do

 ▸ PULL REQUESTS
 ────────────────────────────────────────────────────────────────────
   ↑  !234  payments
     Retry logic for webhook failures
     opened · 0 reviews

   ✓  !228  payments
     Idempotency keys on /charge
     merged

   ⧖  !231  dashboards
     Latency dashboard v2
     awaiting · 0 approvals

 ▸ Sprint 42 · 3 / 14 days
 ────────────────────────────────────────────────────────────────────
   Issues     7 / 11   64%
   ██████████████░░░░░░░░

   Points     18 / 28   64%
   ██████████████░░░░░░░░

   Velocity   1.6 pt/d
   Need       3.3 pt/d

   Cycle (avg, done)
     In Progress     8h
     In Review       3h
     QA              1h
```

### Plain (`jog --plain`)

```
═══════════════════════════════════════════
 Standup — Anthony Norfleet (2026-04-22 09:04)
═══════════════════════════════════════════

Since Tue Apr 21 (2026-04-21 → now):
  • [PROJ-388] OAuth refresh race condition (status: Done)
  • [PROJ-389] Idempotency keys on /charge (status: Done)
      - transitioned: In Review → Done
  • [PROJ-401] Dashboard latency spike (status: In Progress)
      - updated: description
  • [PROJ-412] Refund webhook retry logic (status: In Review)
      - transitioned: In Progress → In Review
      - updated: sprint, story_points
      - commented: "spec covers 409 but not 425 — added test"

Today:
  • [PROJ-420] Backfill legacy accounts (In Progress)
  • [PROJ-425] Investigate 5xx in eu-west-1 (To Do)

Bitbucket:
  Opened:
    • !234 [team/payments] Retry logic for webhook failures (no approvals yet)
  Merged:
    • !228 [team/payments] Idempotency keys on /charge
  Awaiting approval:
    • !231 [team/dashboards] Latency dashboard v2 (no approvals yet)

Sprint:
  Sprint 42 (3 days left of 14)
  Issues: 7/11 done
  Points: 18/28 done (64%)

  Velocity:
    Current:  1.6 pts/day
    Needed:   3.3 pts/day to finish on time

  Avg Cycle Times (completed tickets):
    Created → Done       2d 4h
    To Do → Done         1d 6h
    In Progress          8h
    In Review            3h
    QA                   1h
```

## Commands

| Command      | What it does                                       |
| ------------ | -------------------------------------------------- |
| `jog`        | Print the standup summary (default command)        |
| `jog setup`  | Prompt for credentials and save to OS credential store |
| `jog config` | Show stored credentials (masked) and config path   |

## Troubleshooting

- **`JIRA_API_TOKEN not set and not in Keychain`** — run `jog setup`.
- **Linux: keyring errors on first use** — needs a running Secret Service
  provider (GNOME Keyring, KWallet with secret-service, or KeePassXC with
  its Secret Service integration enabled). On a fresh headless VM, install
  `gnome-keyring` and ensure the session bus is running, or just use env
  vars instead.
- **`/myself failed`** (with `--debug`) — your token may be scoped and
  blocking `/rest/api/3/myself`. Create a non-scoped "Create API token"
  instead, or set `JIRA_ACCOUNT_ID` + `JIRA_DISPLAY_NAME` env vars.
- **Sprint section missing** — no open sprint and no sprint closed within
  the last 2 days, or no assigned issues in either. Check `[jira].projects`
  in `config.toml`.
- **Wrong story-point totals** — custom field IDs differ per Jira instance.
  Set `[fields].story_points` and `[fields].sprint` in `config.toml`.
- **Bitbucket section missing** — either not configured (check `jog config`),
  no PR activity in the window (correct, hides), or the API token lacks
  Bitbucket scopes. Recreate at
  https://id.atlassian.com/manage-profile/security/api-tokens with
  `read:account`, `read:pullrequest:bitbucket`, `read:repository:bitbucket`.
- **Bitbucket run is slow** — we iterate all repos in the workspace (one
  HTTP call per repo) because Atlassian retired the workspace-wide PR
  endpoint. Scales linearly with workspace repo count; capped at 100 repos
  per run. Set Bitbucket project keys during `jog setup` to narrow the
  fan-out.
- **macOS Keychain prompts every run** — "Always Allow" is tied to the
  binary's code signature, which changes on every `cargo build`. For a
  personal dev machine: open **Keychain Access**, search `jog_`, and for
  each entry set Access Control → "Allow all applications to access this
  item". One-time setup, no more prompts.

## Platform

Cross-platform credential storage via the [`keyring`](https://crates.io/crates/keyring)
crate:

- **macOS** — login Keychain (`apple-native` feature). Existing entries
  written by `security add-generic-password` are read transparently.
- **Linux** — Secret Service over DBus (`sync-secret-service` +
  `crypto-rust`). Works with GNOME Keyring, KWallet (secret-service),
  KeePassXC.
- **Windows** — Credential Manager (`windows-native`).

If your platform can't provide a credential store (headless CI, locked-down
servers, etc.), fall back to the `JIRA_*` env vars.
