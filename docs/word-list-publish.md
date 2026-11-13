# Word list: the shared document and the external dictionary

Thoth's word list is one JSON document (the format below) carrying the dictionary and the canonical terms. It has two homes, both off by default, so a public build with neither set is purely local:

- **External dictionary** (**Settings → Integrations → External dictionary**): the household's dictionary service, wordhoard, is the source of truth. This is the read contract.
- **Word list publishing** (WebDAV file): Thoth writes its own file. It remains as the seed for a fresh service (`POST /api/dictionary/import` takes this file) and for setups with no service. It pauses while the external dictionary is on.

## External dictionary

Set the service URL, the device's login and the device's app password (its own identity-provider app password, held in `~/.thoth/wordhoard_password`, owner-only, never in `config.json`). The device name is sent with every write; empty uses the hostname. Turning it on **replaces the local lists** with the service's; the previous ones are saved to `~/.thoth/dictionary_before_external.json`.

| Call | What |
| --- | --- |
| `GET /api/dictionary/word-list` | The version-1 document with an `ETag`. Thoth sends `If-None-Match`; `304` means unchanged. A document with a higher `version` is refused. |
| `POST /api/dictionary/changes` | A batch `{"changes":[…]}` of `entry`, `term`, `ban`, `refusal` changes, each `op: add \| remove`, with `X-Wordhoard-Device`. Thoth sends `entry` and `term` only: it has no bans or refusals. |
| `GET /api/dictionary/rows` | Shared rows plus this device's own bans and refusals: the read-back after a reinstall (the keyboard's; Thoth has nothing device-local to restore). |
| `GET /api/dictionary/history` | Who wrote each row, from which device. |

Auth is HTTP Basic.

- **Read** on start, every 60 seconds, and after each local edit.
- **Write through**: every edit, whatever made it (editor, import, MCP), is sent within about a second as the difference between the last good document and the local lists: removes first, then adds, so a changed row is remove-old, add-new. A failed write stops the sync before the read, so the edit is neither lost nor overwritten; it is retried on the next tick.
- **Offline**: the last good document (`~/.thoth/external_dictionary_last_good.json`) and the local lists are the working copy. A failed request records the error in the settings pane and changes nothing.

The rest of this page is the document format, shared by both homes.

## WebDAV publishing

- **Edits in Thoth reach the file within a few seconds**, whatever made them — the dictionary editor, an import, the MCP `dictionary`/`canonical` tools. A burst of edits becomes one write.
- **Publish now** writes the file immediately.
- **Thoth is the single writer.** Publishing is one-way: the file is never read back, and a change made in the file itself is overwritten by the next publish. If a second writer ever appears, two-way sync can be added then — with a merge policy and its own format version.

The password is stored the way Thoth stores its API token: a dedicated owner-only file in `~/.thoth`, never in the settings file. Nextcloud users should store an **app password** (Settings → Security), not their login password.

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
- Both arrays may be empty. A future format change bumps `version`; a consumer that does not understand the higher version should refuse the document rather than partially read it.

## For a second consumer

The file is plain HTTP with basic auth — no locking protocol, no WebDAV-specific headers:

1. `GET` the URL with basic auth, parse the JSON, apply the entries and terms to your own text path.
2. Read on your own cadence and cache as you like; the file changes only when the lists change.
3. Treat the file as read-only: Thoth overwrites it on the next save and does not merge. If you need to change a correction, change it in Thoth (the app, or its MCP `dictionary`/`canonical` tools).

## Settings reference

| Setting | Where | Default |
| --- | --- | --- |
| Enable word list publishing | Settings → Integrations → Word list publishing | Off |
| File URL, username, password | Same section | empty |
| `publish.enabled` (config key) | `~/.thoth/config.json` → `publish` | `false` |

The MCP `setting` tool can change the `publish` section too (`{"publish":{"enabled":true,"url":"...","username":"..."}}`); the password has no read path, only the settings pane's write-only field.
