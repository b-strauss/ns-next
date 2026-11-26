---
name: watching-a-pull-request
description: Use when an ns-next pull request has just been opened or pushed to, when CI is running on one, when a check of one has failed, or when the user asks how a pull request, its checks or its review comments are going.
---

# Watching a pull request

## Overview

A pull request is watched from the moment it is opened until its CI has
passed and nothing is left to act on, without the user asking for it. The
watch is a recurring `CronCreate` job whose prompt reads the pull request
through the GitHub MCP tools. A cron job hands its prompt to this session,
where the MCP tools are available. A `Monitor` or a background shell loop
runs in a shell, where they are not, so it falls back to `curl` against
the API.

A failed check is fixed at once, without asking: the user decided
(2026-11-26) that a CI failure on an open pull request stops the watch,
is fixed, committed and pushed to the pull request's branch, and the
watch starts again. Everything else -- a review comment, a conflict, the
merge -- still waits for the user's word.

## Setting it up

Right after `mcp__plugin_github_github__create_pull_request` answers, or
after a push to the pull request's branch:

1. Create the job: `CronCreate` (deferred, as `CronDelete`,
   `PushNotification` and the GitHub MCP tools are: load them with
   `ToolSearch`, `select:CronCreate,CronDelete,PushNotification`) with
   `cron: "* * * * *"`, every minute, `recurring: true` and this prompt,
   with the pull request's number:

   > Check PR #<number> of b-strauss/ns-next with the GitHub MCP tools
   > (pull_request_read: get_check_runs, get_comments,
   > get_review_comments, get). Report to the user in German only what
   > changed since the last check: a CI job finished (with its
   > conclusion), a new comment or review, a merge conflict, or the PR
   > merged or closed. Send a PushNotification (German, one line under
   > 200 characters, what to act on first) only for a failed check, a new
   > comment or review, a conflict, the PR merged or closed, and once
   > every check has passed -- never for one check passing while others
   > still run. If nothing changed, say nothing and send nothing. If a
   > check failed:
   > delete this cron job (CronDelete) at once, push the failure, and
   > follow the skill watching-a-pull-request, "When a check fails". When
   > every check run has completed successfully and there is no open
   > comment to act on, delete this cron job, tell the user the CI result
   > and push it. Never merge, and never act on a review comment, without
   > the user's word.

2. Tell the user that the watch runs, what it reports, and that it ends on
   its own.

## When a check fails

1. **Stop the watch:** `CronDelete` the job, so that no tick runs while
   the fix is under way.
2. **Read the log:** `mcp__github-actions__get_job_logs` for the failed
   job; a check run's `id` is the job's id. Find the first real error, not
   the step that reported it.
3. **Fix it on the pull request's branch**, by the repository's rules:
   `superpowers:systematic-debugging` for anything not obvious; the
   gates `CLAUDE.md`'s table names for what the fix changes, the commit
   gate at least; a commit of its own, conventional (`fix:`, `test:`,
   `chore:`), with the attribution trailer. A failure only the CI's
   platform shows (macOS on Apple silicon, Linux) and the local machine
   cannot reproduce is fixed from the log and the code; say so.
4. **Push** it: `git push`, a bare command, never piped.
5. **Start the watch again** with a new `CronCreate` job, the same
   prompt, and tell the user and push what failed, what the fix was and
   that the watch runs again.

If the failure is not the branch's -- an outage of GitHub's runners, a
cache or a download that failed -- re-running is not a fix: report it and
push it, leave the code alone, and start the watch again only once the
user re-ran the job or says so. If a fix would change behaviour, a golden
or a design decision rather than repair what the branch broke, stop after
step 2 and ask.

## Quick reference

| Situation | Do |
|---|---|
| A pull request was just opened | Create the cron job at once |
| A fix was pushed to an open pull request whose job is gone | Create it again |
| A check failed | "When a check fails": stop, read, fix, commit, push, restart |
| A check failed for an infrastructure reason | Report and push; no code change |
| A review comment or a conflict | Report and push; wait for the user's word |
| The user wants the watch to cover conflicts and review comments until the merge, for days | Point them at `/steward`, which only the user can start |
| The session is about to end | Say that the job dies with the session |

## Common mistakes

- **Not watching at all.** Opening the pull request is not the end of the
  step. The checks that run there (Linux, Windows, macOS on Apple silicon)
  are the only place the AArch64 JIT runs natively, so they can fail where
  the local gates passed.
- **Asking before fixing a failed check.** The user's standing decision is
  to fix, commit and push it; asking costs the round trip the watch exists
  to save.
- **Keeping the job running while fixing.** Its ticks would report the
  same failure again, or the new run before the fix is pushed.
- **Polling with `curl` in a `Monitor`.** It reaches only the public API,
  without the MCP tools' logs and review threads, and it stops when the
  monitor expires.
- **Using `gh`.** It is not installed on this machine; every GitHub
  operation goes through the MCP tools.
- **Reporting every tick, or pushing every green check.** A tick that
  changed nothing says nothing. A check that passed while others still
  run is said, not pushed: the user's phone (`PushNotification`, through
  the Claude app when Remote Control is connected) gets only what to act
  on -- a failure, a comment, a conflict, the merge -- and the one message
  that every check has passed.
- **Acting on a review comment unasked.** A comment is information, not an
  instruction; report it and wait for the user's word.
