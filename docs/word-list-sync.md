# Word list sync

Optional, off by default: keep the dictionary and the canonical terms in step with one shared file on a WebDAV server (Nextcloud, ownCloud, any plain WebDAV), so other tools — a phone keyboard, a gateway — read and write the same corrections. Until you turn it on in **Settings → Integrations → Word list sync**, the built-in local lists are the whole experience and nothing leaves the machine.

This page is also the format contract for a second consumer: anything that can GET and PUT the file with basic auth can share the list.

## What it does

- **Edits in Thoth reach the file** within 5 seconds, whatever made them — the dictionary editor, an import, the MCP `dictionary`/`canonical` tools.
- **Edits in the file reach Thoth** on the next poll, within about a minute — or immediately if you press **Sync now**.
- **Conflicts resolve in one sentence:** when the same entry was changed on both sides since the last sync, Thoth's version wins; every other change — additions, edits, deletions — propagates from whichever side made it.

The sync keeps a private snapshot (`~/.thoth/sync_state.json`) of the last document both sides agreed on and does a three-way merge against it. Deleting an entry in Thoth deletes it in the file, and vice versa. Changing the URL starts a fresh merge with no deletions: Thoth's rows and the file's rows are unioned, Thoth winning any key both have.

The password is stored where a desktop app keeps one — the OS keyring on Linux and Windows, an owner-only file in `~/.thoth` on macOS — and never in the settings file or the shared file. Nextcloud users should store an **app password** (Settings → Security), not their login password.

## The file format

One JSON document, version 1, carrying both lists:

```json
{
  "version": 1,
  "entries": [
    { "from": "teh", "to": "the", "caseSensitive": false }
  ],
  "canonicalTerms": [
    {
      "term": "LiteLLM",
      "aliases": ["lite llm"],
      "policy": "phonetic",
      "maxWords": 2,
      "threshold": null
    }
  ]
}
```

- `entries` — the flat find/replace dictionary, applied to transcriptions before canonical snapping. Rows are exactly what Thoth's dictionary export returns; `caseSensitive` may be omitted (defaults `false`).
- `canonicalTerms` — the canonical term registry (see [dictionary.md](dictionary.md) for what the fields mean). All fields except `term` are optional and default as in the registry file; `policy` is one of `aliasOnly` (default), `phonetic`, `conservative`.
- Both arrays may be omitted or empty; a missing `version` reads as 1. A file with a **higher** `version` is refused rather than partially read.
- Empty rows (a blank `from`, `to` or `term`) and duplicate keys are dropped on read, first occurrence winning; keys are case-insensitive (`from`, `term`).

## For a second consumer

The file is plain HTTP with basic auth — no locking protocol, no WebDAV-specific headers:

1. `GET` the URL with basic auth, parse the JSON, apply the entries and terms to your own text path.
2. To change the list, merge your edits into the document you just read and `PUT` the whole document back with `Content-Type: application/json`. Thoth picks the change up within a minute.
3. Read on your own cadence and accept that a concurrent write can be overwritten by the other side's next push — the merge contract above is the only guarantee, and it is per entry, last-agreed-state based.

The document is the same bytes Thoth writes for itself, so a consumer that only reads never needs to understand the merge; one that writes only appends or edits rows in the arrays.

## Settings reference

| Setting | Where | Default |
| --- | --- | --- |
| Enable word list sync | Settings → Integrations → Word list sync | Off |
| File URL, username, password | Same section | empty |
| `sync.enabled` (config key) | `~/.thoth/config.json` → `sync` | `false` |

The MCP `setting` tool can change the `sync` section too (`{"sync":{"enabled":true,"url":"...","username":"..."}}`); the password has no read path, only the settings pane's write-only field.
