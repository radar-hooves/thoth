# Personal dictionary and canonical terms

How Thoth fixes the words a transcriber keeps getting wrong, from simple find-and-replace to a smarter system that learns a word once and snaps all its misheard variants back to it.

## The problem

Speech-to-text engines are good at everyday English but poor at the words that matter most to you: people's names, technical jargon, product names, and acronyms. They hear the sound correctly but write down the wrong word. You ask for "portcullis" and get "port colours"; you say "LiteLLM" and get "light LLM"; you mention the product "immich" and it writes "image".

Fixing these by hand after every dictation is tedious, so Thoth corrects them automatically as part of its pipeline. There are two layers that do this, and they run one after the other on every transcription: the flat dictionary first, then the canonical-term registry.

## Layer one: flat replacements

The flat dictionary is a list of "from" and "to" pairs. Whenever the "from" text appears in a transcription, Thoth replaces it with the "to" text. For example, `teh` becomes `the`, or `kubernetes` becomes `Kubernetes`.

Two things worth knowing about how matching works:

- **Whole words only.** An entry `hook` to `look` rewrites the standalone word "hook" but leaves "webhook" untouched. This stops a short entry from accidentally chewing up the middle of a longer word. Multi-word entries (such as `machine learning` to `ML`) match the whole phrase.
- **Case sensitivity is per entry.** Each entry has a case-sensitive toggle. With it off (the default), the entry matches regardless of capitalisation. With it on, only the exact-case word is rewritten; "Hello" and "HELLO" are left alone if your entry is lower-case "hello".

Replacements are literal text, not patterns; there is no regular-expression or wildcard support. The list is applied top to bottom on every transcription before any AI enhancement runs.

The flat dictionary is stored at `~/.thoth/dictionary.json`.

## Layer two: the canonical term registry

The flat dictionary has a weakness: a transcriber can mangle the same word a dozen different ways, and each variant needs its own "from" entry. "portcullis" alone might need entries for "port cullis", "portculis", "port colours", "port collars", and more. That is a lot of upkeep, and you only ever discover a new variant when it slips through.

The canonical registry fixes this. You register a term **once**, and Thoth automatically snaps acoustic and spelling variants back to it, without you having to predict every mangling in advance. "Canonical" just means the one correct spelling you want everything to collapse to.

How aggressively it does this is controlled by a per-term safety setting called the **snap policy**. There are three:

### AliasOnly (the safe default)

Snaps only when the heard text exactly matches the canonical term or one of its explicitly listed aliases (case is ignored). It never guesses. This is identical in behaviour to the old flat dictionary, which is why it is the default; turning the registry on changes nothing until you opt a specific term into a smarter policy. Use this when you want full manual control.

### Phonetic (aggressive)

Snaps when an unknown word both **sounds like** the term and is **spelled similarly** to it. This is the right policy for coined or invented names that you never say as ordinary words: portcullis, LiteLLM, Vaultwarden. Because you never genuinely mean "port colours" in those contexts, it is safe to let Thoth pull anything close back to the registered spelling.

### Conservative (strict)

Uses the same sound-alike-and-spelled-alike test as Phonetic, but demands a much closer spelling match before it acts. This is for names that collide with real, common words. The classic case is a product called "immich" versus the everyday word "image": you want "immich" corrected when you mean the product, but you must never have ordinary uses of "image" rewritten. Conservative keeps the bar high so genuine words are left alone.

### Why both layers require "sounds like AND spelled like"

The Phonetic and Conservative policies do not act on a sound-alike match alone. They require **both** a phonetic match and a minimum spelling similarity. This "AND gate" is deliberate and it fixed a real bug.

The phonetic matcher reduces words to a short code based on how they sound. That code is coarse: "folder" and "filter" both reduce to the same code as "Vaultwarden". On a sound-alike-only rule, registering "Vaultwarden" with an aggressive policy would have started rewriting the ordinary words "folder" and "filter" into "Vaultwarden", which is plainly wrong. Adding the spelling-similarity requirement blocks this: "folder" sounds vaguely like "Vaultwarden" but is spelled nothing like it, so it is left alone. Only words that clear both bars get snapped.

The difference between the two smart policies is just how high the spelling bar is set. Phonetic uses a lower default similarity threshold (around 0.55 on a 0-to-1 scale) so it catches genuine mishearings of invented words; Conservative uses a much higher one (around 0.85) so only near-identical spellings of word-colliding names are touched. You can also override the threshold per term if you need finer control.

A term also has a "max words" setting (default 3) that controls how many consecutive words Thoth will consider as one candidate phrase, so multi-word names like "port cullis" can be matched.

Casing is handled sensibly: if your canonical term has deliberate mixed or upper-case letters (LiteLLM, Vaultwarden) it is inserted exactly as written; if it is all lower-case (portcullis), Thoth follows the capitalisation of the words it replaced.

The registry is stored at `~/.thoth/canonical_terms.json`.

## How your existing dictionary migrates

You do not have to rebuild anything. The first time the canonical registry runs and finds no registry file, it seeds itself from your existing flat dictionary. Entries that share the same "to" value are grouped into a single canonical term, with the various "from" values attached as its aliases. Every seeded term is given the AliasOnly policy, so behaviour is unchanged on day one. The seeded registry is then saved, so this happens only once.

From there you can opt individual terms into the Phonetic or Conservative policy as you decide which ones are safe to match more aggressively. Your flat dictionary is left in place and still applies first.

## Import and export

The flat dictionary can be exported to and imported from a JSON file, which is the easy way to back it up or move it between machines. In the dictionary editor there are import and export buttons; export writes the current list to a file you choose, and import reads entries from a file. Import can either merge (add new entries, skipping any whose "from" already exists) or replace the whole list.

The canonical registry is a plain JSON file at `~/.thoth/canonical_terms.json`, so backing it up is a matter of copying that file.

There is also an optional word-list sync that keeps **both** lists in step with a shared file on a WebDAV server, so other tools can use the same corrections; see [word-list-sync.md](word-list-sync.md).

## Managing it

### Through the app

The flat dictionary has a full editor in Thoth's settings. You can add, edit, delete, and sort entries, set the case-sensitive flag on each one, and import or export the list. Whether the dictionary is applied at all is governed by a toggle in the output-filter settings (it is on by default).

The canonical registry does not yet have its own settings panel in the app. For now it is managed through the MCP tool below, or by editing `~/.thoth/canonical_terms.json` directly (it is human-readable JSON). When seeded from your dictionary it works automatically; you only need to touch it to change a term's policy.

### Through the MCP `canonical` tool

Thoth exposes an MCP (Model Context Protocol) server so an AI assistant can manage it for you in plain language; see [automation.md](automation.md) for setup and the full tool list. The relevant tool here is `canonical`, which manages the registry with four actions:

- **list**: show every registered term with its aliases and policy.
- **add**: register a new canonical term (with optional aliases and a policy; the policy defaults to AliasOnly).
- **update**: change an existing term by its position in the list.
- **remove**: delete a term by its position.
- **suggest**: read your transcription history and report spellings that look like a registered term but were not snapped to it — the mis-hearings still getting through.

The policy values you pass are `aliasOnly`, `phonetic`, or `conservative`. So you can simply ask your assistant something like "register LiteLLM as a canonical term with the phonetic policy" and it will call this tool for you.

### Finding what is still getting through

The registry only fixes mis-hearings you have told it about, and you learn about a new one by noticing it in pasted text. `suggest` does that noticing for you: it scans your recent transcriptions for spellings close to a term you have already registered, and reports each one with how often it occurs, how often the correct spelling occurs, and a line of surrounding text.

It changes nothing. Some rows will be ordinary English words that merely resemble your term, and the context line is there so you can tell at a glance — a "Mac Studio" is not a bad transcription of `stdio`. Ask your assistant to run it and add the real ones, and the list you maintain by hand stops growing by hand.

Two shapes it looks for. One is a spelling close enough to the term to be a plain mis-hearing. The other is the term heard correctly and then extended — "Claude" becoming "Claudette" — which no phonetic comparison catches, because the extra syllable changes the sound. It ignores ordinary inflections (`symlinks`, `symlinked`), which are word forms rather than errors, and anything the ASR produces more often than the correct spelling, which is almost always a real word.

The flat dictionary has its own `dictionary` MCP tool (list, add, update, delete, import, export) covered in [automation.md](automation.md).

## Practical recommendations

- Start with the flat dictionary for exact, predictable corrections (`teh` to `the`, fixed capitalisations).
- Promote a term to **Phonetic** only when it is a coined or invented name you never use as an ordinary word; this is where the registry saves you the most upkeep.
- Use **Conservative** for any name that is also a real English word, so you do not corrupt normal text.
- Leave a term on **AliasOnly** if you would rather list its variants by hand than risk any automatic guessing.
- Run **suggest** every so often rather than waiting to spot a bad transcription yourself; it is the cheapest way to keep the registry current.

## See also

- [Getting started](getting-started.md): install Thoth and record your first dictation.
- [Word list sync](word-list-sync.md): share these lists with other tools through a WebDAV file.
- [Custom prompts](custom-prompts-guide.md): shape how AI enhancement rewrites your text.
- [Automation](automation.md): control Thoth from an AI assistant via the MCP server.
- [Troubleshooting](troubleshooting.md): when corrections are not being applied.
