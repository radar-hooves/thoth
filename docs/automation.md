# Automation: the control API and MCP server

Thoth runs a small local control surface so other programs (especially LLM assistants) can drive it: read and edit your dictionary, change settings, browse history, and transcribe audio files. It has two faces on the same local port: a plain HTTP **control API** for scripts, and a bundled **MCP server** (Model Context Protocol) for assistants like Claude.

## Safety model

This surface is designed to be safe to leave on:

- **Loopback only.** It binds to `127.0.0.1` and is never exposed to the network. Nothing outside your machine can reach it.
- **Token-authenticated.** Every request must carry a bearer token of the form `sk-thoth-…` (generated from your OS's secure random source on first run). Requests without it are rejected.
- **On by default, but local-only.** Both the control API and the MCP server are enabled out of the box so a local assistant works immediately; because they are loopback + token-gated, this is not a network exposure. You can turn either off in the **Integrations** pane, and the toggle takes effect immediately (no restart).

Default address: `http://127.0.0.1:8765`. You can change the port in the Integrations pane if 8765 clashes with something else.

## Your token

Open **Settings › Integrations** to view, reveal, copy, or rotate the token. Rotating it invalidates the old one immediately, so any connected client must be updated with the new value. The token also lives in `~/.thoth/config.json`; treat it like a password.

A quick health check from a terminal (no token needed for `/health`):

```bash
curl http://127.0.0.1:8765/health
```

## Connecting an LLM assistant (MCP)

The MCP server is mounted at **`/mcp`** and speaks the streamable-HTTP transport, behind the same bearer auth. Point your assistant's MCP client at it with the token in an `Authorization` header. The exact config shape depends on the client; a typical entry looks like:

```json
{
  "mcpServers": {
    "thoth": {
      "type": "http",
      "url": "http://127.0.0.1:8765/mcp",
      "headers": { "Authorization": "Bearer sk-thoth-REPLACE_WITH_YOUR_TOKEN" }
    }
  }
}
```

Once connected, the assistant can call these tools:

| Tool                       | What it does                                                                             |
| -------------------------- | ---------------------------------------------------------------------------------------- |
| `dictionary`               | List, add, update, delete, import, or export flat dictionary entries                     |
| `canonical`                | List, add, update, remove, or suggest canonical terms (see the [Dictionary guide](dictionary.md)) |
| `setting`                  | Read or change a setting                                                                 |
| `transcription`            | List past transcriptions, fetch one, or get stats                                        |
| `transcribe_file`          | Transcribe a local audio file as a background job                                        |
| `transcribe_status`        | Check on a `transcribe_file` job                                                         |
| `get_state` / `get_system` | Read the app's current state and system/model info                                       |
| `list_prompts`             | List the available AI-enhancement prompts                                                |

Destructive and system actions (starting a recording, changing audio devices, quitting) are intentionally not exposed; the surface mirrors what you can do safely in the GUI.

For example, you can ask your assistant to "add _portcullis_ to my Thoth dictionary so it stops being misheard", or "transcribe `/Users/you/voice-memo.m4a` and summarise it"; the assistant calls `canonical`/`dictionary` or `transcribe_file` on your behalf. `transcribe_file` needs an **absolute** path — a `~`-relative one is rejected even though the tool describes itself as accepting one. `get_state` reflects only the live-recording pipeline (idle/recording/processing) and does not track a `transcribe_file` background job's progress; poll `transcribe_status` for that instead.

## Driving it from a script (control API)

If you'd rather script it directly, the same endpoints are available over HTTP with the bearer token. A few read-only examples:

```bash
TOKEN="sk-thoth-REPLACE_WITH_YOUR_TOKEN"
BASE="http://127.0.0.1:8765"

curl -H "Authorization: Bearer $TOKEN" "$BASE/state"
curl -H "Authorization: Bearer $TOKEN" "$BASE/dictionary"
curl -H "Authorization: Bearer $TOKEN" "$BASE/prompts"
```

Writes use the matching verbs (for example `POST /dictionary` to add an entry, `PATCH /settings` to change a setting, `POST /transcribe` to queue a file). If a request is missing or has the wrong token it returns an authentication error; if the port is already in use, Thoth surfaces the error rather than failing silently.

## Turning it off

If you don't use automation, open **Settings › Integrations** and switch off the MCP server, the control API, or both. They stop accepting connections immediately.

## See also

- [Dictionary and smart correction](dictionary.md): what the `dictionary` and `canonical` tools manage
- [AI enhancement prompts](custom-prompts-guide.md): what `list_prompts` returns
- [Troubleshooting](troubleshooting.md): if a client can't connect
