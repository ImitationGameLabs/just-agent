# `just-agent-run` Reference

Agent runner for scripting and automation. Creates an agent,
streams progress to stderr, prints the final result to stdout, and exits
with a semantic exit code. Designed for scripted and automated workflows
where the caller needs machine-readable output and exit-status-driven
control flow.

By default the agent is **preserved** after completion so that logs,
history, and token usage remain available for auditing. Pass `--delete`
to remove the agent and all associated data after the run finishes.

```bash
just-agent-run [OPTIONS] <PROMPT>
```

Uses `JUST_AGENT_AUTH_TOKEN` (mandatory) and `JUST_AGENT_DAEMON_URL`
(env, default `http://127.0.0.1:3000`).

## Arguments

| Argument   | Description                                |
| ---------- | ------------------------------------------ |
| `<PROMPT>` | The prompt to send to the agent (required) |

## Options

| Flag                     | Description                                               |
| ------------------------ | --------------------------------------------------------- |
| `--workspace-root <DIR>` | Working directory for the agent                           |
| `--max-rounds <N>`       | Maximum tool-call rounds (overrides daemon default)       |
| `--delete`               | Delete the agent and all associated data after completion |

## Exit codes

| Code | Meaning               |
| ---- | --------------------- |
| 0    | Success               |
| 1    | Error                 |
| 2    | Max rounds exceeded   |
| 3    | Cancelled             |
| 4    | Token budget exceeded |

## Agent retention

By default the agent is **preserved** after the run completes. The agent ID
is printed to stderr so you can inspect logs, token usage, and history later:

```
$ just-agent-run "Summarize the project"
[tool] read_file
[tool-result] ...
The project is a ...
agent a3f1b2c4-5678-90ab-cdef-1234567890ab finished (kept). Use `just-agent stop a3f1b2c4-5678-90ab-cdef-1234567890ab` to delete.
```

Use `--delete` to clean up the agent immediately after the run:

```bash
just-agent-run --delete "Summarize the project"
```

For the complete environment variable reference including LLM provider
configuration, see [env.md](env.md).
