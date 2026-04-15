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
```

Find your custom field IDs at
`https://<your-org>.atlassian.net/rest/api/3/field` (look for names like
"Story Points" and "Sprint").

## Run

```bash
jog                         # previous work day + today so far
jog --date 2026-04-10       # override the start date
jog --format markdown       # text (default) | markdown | json
jog --debug                 # print JQL, window, issue counts, config path
```

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

```
═══════════════════════════════════════════
 Standup — Jane Doe (2026-04-14)
═══════════════════════════════════════════

Since yesterday:
  • [PROJ-123] Refactor auth middleware (status: In Review)
      - transitioned: In Progress → In Review
      - commented: "Pushed PR, ready for review"
  • [PROJ-145] Flaky test in checkout (status: In Progress)
      - updated: description

Today:
  • [PROJ-145] Flaky test in checkout (In Progress)
  • [PROJ-162] Migrate logging to structured JSON (To Do)

Sprint:
  Sprint 42 (3 days left of 14)
  Points: 18/28 done (64%)
  Issues: 7/11 done

  Velocity:
    Current:  1.6 pts/day
    Needed:   3.3 pts/day to finish on time

  Avg Cycle Times (completed tickets):
    Created → Done        2d 4h
    To Do → Done          1d 6h
    In Progress           8h
    In Review             3h
    QA                    1h

Blockers:
  • <fill in or 'none'>
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
