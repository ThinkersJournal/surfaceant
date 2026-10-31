# surfaceant HANDOFF

You are the `surfaceant` lane (cwd `C:\Projects\surfaceant`, repo `ThinkersJournal/surfaceant`). Brief: `C:\Projects\.lane-state\newlane\surfaceant.md`.
Read `C:\Projects\CLAUDE.md` and `C:\Projects\AGENT-ONBOARDING.md` once.

## State (2026-10-31)
- PR #1 (ROADMAP.md, docs/DESIGN.md, stub 0.0.1) and PR #2 (docs/SPIKE-WORKER-LANGUAGE.md + spike/ts, spike/rust, stub 0.0.2): the PM
  said both may merge; check `gh pr view` for state.
- Spike result: Rust (workers-rs 0.8.7) passes everything incl. HMAC, alarms, capped body; recommendation Rust. TS is the fallback.
- PM [HOLD], CireSnave verbatim: "Hold on.  Lets look at our total stack and decide what shape our agent stack should be before we start building it."
  **No Worker code until the PM says.**
- PM Q2-Q4 rulings (still valid): admin-provisioned channels in v1; channel id = rand || HMAC (key = Worker secret) so unknown ids 404
  without touching a DO; caps as Worker vars with the DESIGN.md defaults.
- CI job names for branch protection were sent to the PM. If a `worker` job is added later, tell the PM.
- Scratch, uncommitted: worktree `C:\Projects\surfaceant-wt-worker` (TS Worker skeleton; fallback only). Remove it if TS is not chosen.

## Next
1. Wait for the PM. The only unblocked work: after #1/#2 merge, remove worktrees `surfaceant-wt-design` and `surfaceant-wt-spike`
   (`git status --porcelain` first) and delete the remote branches (CLAUDE.md section 3).
2. On the PM's go and the language ruling: row 2 (Worker + DO skeleton with tests), then row 3 (Rust client + e2e against
   `tools/gh_webhook_receiver.py`).
3. Amend DESIGN.md for the rulings above (HMAC ids, Worker vars) and for the language choice once made.
