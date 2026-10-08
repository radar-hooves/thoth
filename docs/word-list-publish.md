# Word list publishing

Optional, off by default: write the dictionary and the canonical terms to one shared file on a WebDAV server (Nextcloud, ownCloud, any plain WebDAV), so other tools — a phone keyboard, a gateway — can read the same corrections. Until you turn it on in **Settings → Integrations → Word list publishing**, the built-in local lists are the whole experience and nothing leaves the machine.

This page is also the format contract for a second consumer: anything that can GET the file with basic auth can read the list.

## What it does

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
