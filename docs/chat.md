# Durable terminal chat

`polkagent chat` is the line-oriented terminal surface over Polkagent's durable
interaction service. It complements the full-screen `polkagent tui` and the
editor-facing `polkagent acp` transport.

## Start or resume a session

Select one active agent by name or ID:

```bash
polkagent chat --agent research-agent
```

The command creates a durable interaction and writes its ID to stderr as
`chat session: <conversation-id>`. Resume it in a later process:

```bash
polkagent chat --agent research-agent --resume <conversation-id>
```

For a new interaction, `--agent` chooses the initial active target. On resume,
the durable interaction target is authoritative: a target previously selected
with `/agent` is restored even when the process was started with a different
`--agent` value. Chat prints the persisted user/assistant transcript before
accepting more input.
Transcript reads go through the surface-neutral interaction service as bounded,
ordinal pages. Each item is correlated to its durable turn and exact user and
optional assistant message identity; broken, mismatched, or rich-content
correlations fail closed instead of being guessed from message order. Active
turns show their user text without inventing an assistant response.
The SQLite persistence adapter applies the requested ordinal limit/offset
before loading those exact message IDs, so an unrelated or very large
conversation history is not scanned into each transcript page.
Group and automatic targets are deliberately refused; terminal chat does not
pretend that group orchestration is wired when it is not.

Interactive prompts may span multiple lines. Enter a blank line to submit the
buffer. EOF submits a non-empty pending buffer and then exits. At an idle
prompt, Ctrl-C exits while retaining the durable session. During a turn,
Ctrl-C or `/cancel` cancels only that active durable turn and leaves the
interactive session usable.

Terminal chat intentionally uses cooked, line-oriented input. It never enables
raw mode or the alternate screen, so an error or panic cannot strand the
terminal in a modified mode.

## Slash commands

Slash commands are parsed by the shared interaction command registry and the
supported commands execute through its shared service executor:

- `/help [command]` — show the truthful terminal command surface.
- `/status` — show the durable session, target, turn count, and active work.
- `/agents` — list active targets from the retained runtime registry. The list
  reports durable lifecycle only; exact per-agent provider/model readiness is
  not currently projected and is not guessed.
- `/agent <name-or-id>` — validate an exact active name or UUID and persist it
  as this conversation's target. Unknown, inactive, and ambiguous selectors
  fail closed. A conversation with non-terminal durable work cannot switch.
- `/cancel [all]` (alias `/stop`) — cancel the active turn or all active turns
  in the selected interaction.
- `/new [title]` — create and select another durable interaction for the
  currently selected agent.
- `/resume <conversation-id>` — select a durable single-agent interaction,
  adopt its persisted target, and print its transcript.
- `/model [model-id]` — show the current effective conversation model, or
  validate and persist a same-provider model for this conversation. The
  selection survives process restart and resume without changing the agent.

The following capabilities are explicitly unavailable rather than simulated:

- Provider, harness, or autonomy changes: configure the runtime or agent and
  restart. `/provider`, `/harness`, and `/autonomy` return an error.
- Approvals (`/approve`, `/deny`): use the durable `polkagent inbox` commands.
- Run listing/inspection and run-ID cancellation: use the top-level inspection
  surfaces. Terminal `/cancel` is turn-scoped.
- Group orchestration and rich resource input.

Approval visibility is also reported as unavailable in `/status`; a synthetic
zero is not presented as authoritative.

Model selection goes through the interaction service's typed configuration
path. Unknown models, models belonging to another provider, and dynamic model
changes under a harness backend are refused with typed errors. Successful
selection is scoped to the selected durable conversation, does not mutate the
shared agent specification, and never creates a transcript turn. `/model`
without an argument reports the persisted selection (or the runtime/agent
default when no override exists).

Agent selection follows the same rule: the shared command executor calls the
interaction service's typed target-configuration path. It never mutates a
shared `AgentSpec`, never opens a second registry, and never creates a prompt
turn, run, or interaction event. The service remains authoritative for target
and model compatibility, so a target change can still be refused when the
conversation's persisted model is invalid for that agent. Successful target
selection survives process restart because it is stored on the conversation.

## Pipes and output contract

When stdin is not a TTY, the complete stdin body is one prompt. Internal
newlines are retained and trailing CR/LF delimiters are removed:

```bash
printf 'Compare these two plans\nKeep the answer brief\n' |
  polkagent chat --agent research-agent
```

Non-interactive mode performs one prompt or slash command and exits. An empty
stdin with `--resume` prints the durable transcript and exits.

- stdout contains assistant text, requested slash-command output, or a resumed
  transcript. Streaming deltas are reconciled with the durable terminal result
  so text is not duplicated.
- stderr contains session/turn IDs, runtime readiness and degradation notices,
  lifecycle changes, usage, cancellation progress, and safe typed errors.
- Interactive input markers (`you>` and `...`) are written to stderr, so
  redirecting stdout still yields content without terminal chrome.

If the bounded live event stream lags, chat re-subscribes through the durable
interaction service after its last delivered checkpoint and replays the gap.
It does not abandon still-active work merely because a live receiver fell
behind.

For executor-backed follow-ups, Polkagent scans at most the latest 1,000 prior
turn records, retains the newest 32 completed user/assistant pairs, and sends
those pairs in chronological order before the current prompt. Failed,
cancelled, timed-out, pending, and partial turns are omitted as whole pairs.
Contextual history for harness execution remains unsupported rather than being
inferred from the rendered transcript.

For Zed and other ACP-capable editors, use `polkagent acp`; see
[`acp-zed.md`](acp-zed.md). ACP owns stdout as JSON-RPC transport, whereas
terminal chat's stdout contract is human/text-stream oriented.
