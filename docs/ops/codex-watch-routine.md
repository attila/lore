Hey — Attila here. This scheduled routine monitors whether the lore Codex plugin can be unblocked. It posts to Slack ONLY when there is actionable movement — something that changes what I should do next. Silence is the normal outcome and is correct.

The gate is the recommended action, not the evidence. If today's recommended action is the same one you last sent me, there is no post — no matter how much new evidence supports it, and no matter how important that evidence is. `additionalContextLimit = 0` was worth telling me once, on 2026-07-21. It was sent sixteen days running, which trained me to skim past the one post that mattered.

Never lead with a standing fact. The headline is always the newest qualifying event, with its date. Background belongs in the rolling summary at the bottom, never in the headline and never above the fold.

Context: lore's Codex plugin is parked because Codex hook `additionalContext` is model-visible but also human/transcript-visible. PR #31252 improved the TUI by truncating long hook context, but that is NOT enough: full context remains visible via Ctrl+T/raw history. The unblock is either:
1. a supported model-visible, TUI/transcript-hidden hook context path, or
2. a documented `UserInstructions`/equivalent hidden instruction surface suitable for lore's injected conventions.

Watch these primary items:
• https://github.com/openai/codex/issues/16933
• https://github.com/openai/codex/issues/21696
• https://github.com/openai/codex/issues/22861
• https://github.com/openai/codex/issues/20766
• https://github.com/openai/codex/pull/30138
• https://github.com/openai/codex/pull/30139
• https://github.com/openai/codex/pull/30141

The watch set extends itself. Any PR that cross-references a watched issue, or that a staff comment names as a successor to watched work, joins the set permanently via `watch_extra` in state. The primary list above went stale once already — #30138/#30139/#30141 have been closed since 2026-07-09, while #31885–#31887 and #34393, the items that actually moved, were never on it. Do not let that happen again.

Also watch for PRs/releases mentioning:
`additionalContext`, `hookSpecificOutput`, `hook context`, `hidden from TUI`, `transcript`, `raw history`, `suppressOutput`, `visibilityHint`, `visibility_hint`, `hideInTui`, `tuiVisible`, `model-visible`, `UserInstructions`.

0. Load state.

State is the memory of what you have already been told. It lives in a private gist and is the only thing that makes "already reported" enforceable. Roughly one run in six fails outright — there was no post on 07-08, 07-09, 07-12, 07-15 or 08-01 — so a time window alone would silently drop events with no way to recover them.

```sh
STATE_GIST="<gist id>"                       # private gist, one file: state.json
gh gist view "$STATE_GIST" -f state.json > /tmp/state.json 2>/dev/null \
  || echo '{"last_post_date":null,"last_action_id":null,"reported_events":[],"watch_extra":[],"last_movement":null}' > /tmp/state.json
jq . /tmp/state.json
```

Schema:
• `last_action_id` — slug of the last action sent to Slack, e.g. `adopt-additionalContextLimit-0`. The primary dedupe key.
• `reported_events` — event IDs already reported, e.g. `pr:34393:merged`, `issue:21696:comment:4960757280`.
• `watch_extra` — issue/PR numbers added by the self-extension rule.
• `last_movement` — `{date, what}` for the rolling summary.

If the gist cannot be read, abort the run. Do not fall back to posting. A run without memory cannot tell new from old, and that is exactly the failure being fixed.

1. Check watched issues and PRs.

This gathers standing state for the rolling summary. It decides nothing on its own — step 4 applies the gate.

```sh
WATCH="16933 21696 22861 20766 30138 30139 30141 $(jq -r '.watch_extra[]?' /tmp/state.json)"
for item in $WATCH; do
  gh issue view "$item" --repo openai/codex --json number,title,state,closedAt,updatedAt,comments,timelineItems,url --jq '
    {
      number,
      title,
      state,
      closedAt,
      updatedAt,
      url,
      latest_comment: (.comments | last | if . == null then null else {author: .author.login, createdAt, url, body: (.body[:500])} end),
      openai_comments: [.comments[]? | select((.author.login | test("oai|openai"; "i"))) | {author: .author.login, createdAt, url, body: (.body[:300])}],
      upstream_pr_refs: [.timelineItems[]? | select(.__typename == "CrossReferencedEvent") | select(.source.__typename == "PullRequest") | select(.source.repository.nameWithOwner == "openai/codex") | {number: .source.number, title: .source.title, state: .source.state, url: .source.url, createdAt: .createdAt}]
    }'
done
```

Every `upstream_pr_refs` number not already in `watch_extra` is added to it in step 6.

2. Search for newly relevant open or recently updated upstream PRs.

```sh
SINCE_DATE=$(jq -r '.last_post_date // "2026-07-06"' /tmp/state.json)
gh search prs --repo openai/codex --updated ">=$SINCE_DATE" \
  'additionalContext OR hookSpecificOutput OR "hook context" OR suppressOutput OR visibilityHint OR visibility_hint OR hideInTui OR tuiVisible OR "hidden from TUI" OR UserInstructions' \
  --json number,title,state,author,createdAt,updatedAt,closedAt,url --limit 30
```

3. Check recent releases for relevant changelog entries.

```sh
gh release list --repo openai/codex --limit 15 --json tagName,publishedAt,body --jq --arg since "$SINCE_DATE" '
  [.[] |
    select(.publishedAt > $since) |
    {
      tag: .tagName,
      publishedAt,
      hits: ([.body | split("\n") | .[] |
        select(test("additionalContext|hookSpecificOutput|hook context|hidden from TUI|transcript|raw history|suppressOutput|visibilityHint|visibility_hint|hideInTui|tuiVisible|model-visible|UserInstructions"; "i"))
      ])
    } |
    select(.hits | length > 0)
  ]'
```

4. Classify, then decide whether to notify.

Build the candidate list: every event found in steps 1–3 whose ID is not already in `reported_events`. Anything already reported is finished — drop it silently, however significant it was when it first landed.

Give every surviving candidate exactly one class:

• **A — unblocks lore.** A model-visible/TUI-hidden context path, or an equivalent hidden instruction surface, reaches a *released* version. Merged is not released; "in an alpha" is class A only if the alpha is installable today.
• **B — changes what lore must build.** A maintainer states a direction or decision, an approach is relocated or revived, or a watched blocker closes. The 2026-08-04 finding — async hooks moving to `codex-internal#1395–#1397` — is class B, and it should have been a headline instead of a third bullet under a fifteen-day-old spill-limit story.
• **C — needs a reply or decision from you.** A maintainer answers your 2026-07-13 direction ask on #21696, or asks you something, or requests a repro.
• **D — context only.** Everything else: cross-references, shelvings with no successor, community patches awaiting review, `updatedAt` churn, releases that touch the surface without changing what lore can do.

Classes A, B and C may post. **Class D never posts.** It goes to the rolling summary and the dashboard, where you will see it when you look, and nowhere else.

Then apply the action gate, which overrides everything above:

Derive `action_id` — a stable slug for what you are asking me to do (`adopt-additionalContextLimit-0`, `reply-on-21696`, `retest-hook-visibility-v0.146`, `none`). **If `action_id` equals `last_action_id` in state, do not post.** Same ask, already sent. This holds even for class A. New evidence for an action you have already been given is not a reason to interrupt me again.

Do NOT notify for these by themselves:
• #31252-style truncation only.
• Multiline rendering fixes.
• Generic hook parity updates with no hidden/model-visible context path.
• Community forks or external repo references.
• `gh` errors.
• Any event ID already in `reported_events`.
• Any class D event, alone or in combination. Ten class D events are still not a post.
• A repeat of `last_action_id`, however it is reworded.
• The absence of a response. "Still no maintainer reply", "still parked", "still blocked" is the steady state, not an event.
• Anything you cannot support with a URL a reader can open. No "may surface via", no "likely shipped in", no inference about private repos. `codex-internal` is not readable by this routine; report only what a public comment says about it, quoted and linked.

If nothing survives: do not post to Slack. Write the dashboard "no change" note and stop. Do not send a shortened post, a "no change" post, a summary-only post, or an empty message. Make no `curl` call at all.

If you cannot name, in one line, the single qualifying event and its date, treat that as no trigger and stop.

5. If something changed, post to Slack.

Webhook URL: read from the `LORE_WATCH_SLACK_WEBHOOK` environment variable. Do not inline the URL in this routine or in any committed file.

Slack mrkdwn rules:
• Bold uses `*one asterisk*`.
• Links use `<URL|display text>`.
• Bullets use `• `.
• No mentions.
• 3-5 lines for the movement block, plus the rolling summary block.
• Lead with `🚩`.

Shape:

```text
🚩 *<the newest qualifying event, named plainly>* — <YYYY-MM-DD>
• Evidence: <URL|issue/PR/release> — <one short fact>
• Why it matters: <one clause tying it to lore's unblock>
Action: <the new action, one sentence>.

*Standing status* — <YYYY-MM-DD>
• Blocker: <is hook context still TUI/transcript-visible?>
• Open: #16933 <state> · #21696 <state> · #22861 <state> · #20766 <state>
• In flight: <live approaches with dates, e.g. async hooks → codex-internal#1395–1397>
• Landed: <merged/released items, one line, with dates>
• Since last alert: <class D movement, max two items, or "none">
```

The headline is the class A/B/C event that passed the gate, and nothing else. If you are tempted to headline something that merged two weeks ago, the gate has already failed.

Every evidence bullet carries the date of the event, so a stale fact is visible as stale before it is sent.

`Action` appears only when `action_id` changed. If the action is unchanged you are not posting at all, so the line cannot repeat.

The `Standing status` block is the rolling part: rebuild it from step 1's live data on every post, never copy it forward. It carries the context that used to pad the evidence bullets, so the movement block stays short and strictly about the delta. It is never a reason to post — it only ever rides along with a post the gate has already justified.

Send with:

```sh
SUMMARY="<your formatted summary>"
echo "$SUMMARY" | jq -Rs '{text: .}' | curl -sS -X POST -H 'Content-type: application/json' --data @- "$LORE_WATCH_SLACK_WEBHOOK"
```

A 200 response with body `ok` means delivered. Post at most once per run.

6. Save state — only after a confirmed `ok`.

```sh
jq --arg d "$(date -u +%F)" --arg a "$ACTION_ID" \
   --argjson ev "$NEW_EVENT_IDS" --argjson wx "$NEW_WATCH_EXTRA" --argjson lm "$LAST_MOVEMENT" '
  .last_post_date = $d
  | .last_action_id = $a
  | .reported_events = (.reported_events + $ev | unique)
  | .watch_extra = (.watch_extra + $wx | unique)
  | .last_movement = $lm
' /tmp/state.json > /tmp/state.new.json
gh gist edit "$STATE_GIST" -f state.json /tmp/state.new.json
```

If the post failed, leave state untouched so the next run retries the same event. If the post succeeded but the gist write failed, say so loudly in the dashboard — the next run will duplicate, and that is the one case where a duplicate is expected.

`watch_extra` and `reported_events` accumulate even on class D events, so context-only findings still teach the routine what it has already seen without ever reaching Slack.

Dashboard output must always include:
• the state loaded (`last_post_date`, `last_action_id`), and the candidate list with each item's class
• which class A/B/C event fired with its timestamp, or "no change"
• exact evidence URLs and issue/PR/release numbers
• `action_id`, and whether it matched `last_action_id`
• the standing status block, whether or not anything was posted
• Slack curl response and gist write result if a post was attempted

The dashboard is where "still parked, nothing moved, here is all the class D churn" belongs. Slack is not.

If `gh` errors before change-state is known, do NOT post to Slack. Log the command and error in the dashboard. A failed or partial run is never movement, and never writes state.
