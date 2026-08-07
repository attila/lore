# Agent instructions

Working rules for anyone, human or agent, making changes in this repository.

## Project status

Read `ROADMAP.md` and recent git history before answering what is next. Never answer from memory.
The roadmap is the committed source of truth for status.

## Changelog

Entries are user-facing only. One sentence per change, written in an assertive voice and ending in
`(#N)`. Detail belongs in the pull request body, not here. Dependency bumps and internal refactors
get no entry.

## Roadmap upkeep

A pull request that completes a `ROADMAP.md` entry moves that entry to `## Completed` in the same
diff. No separate tidy-up pull request afterwards.

## Verification

Smoke-test every pull request by hand. A one-off runbook under `tmp/` is not continuous integration
smoke, so do not present it as one. Manual checks stay the rule until a real smoke tier lands.
