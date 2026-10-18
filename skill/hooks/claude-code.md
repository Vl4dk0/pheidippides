# Claude Code: run `phi fetch` at session start (optional)

Pheidippides never depends on hooks. This one only saves the agent a step.
`phi fetch` prints nothing when there is nothing new, so the hook costs ~0 tokens
in the common case, and a one-line summary of unread threads otherwise.

Add to `~/.claude/settings.json` (merge into an existing `hooks` object):

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "phi fetch" }
        ]
      }
    ]
  }
}
```

Claude Code adds the hook's stdout to the session context, so the agent sees the
line and follows the skill (mention it in one sentence, do not derail the task).

## Other harnesses

If your harness has no session-start hook, put this line in `AGENTS.md` /
`CLAUDE.md` instead (see `skill/AGENTS-snippet.md`):

> Run `phi fetch` at session start.
