# Writing scenarios

**For the person who writes tests, not the person who wires them up.**

You describe what should happen, in sentences. Somebody on your team maintains
the *vocabulary* — the list of sentences proef understands — in files called
packs. You never have to open one. This page is your whole loop.

If you also maintain the vocabulary, you want [AUTHORING.md](AUTHORING.md)
instead; this page deliberately stops at the boundary.

## 1. A test is sentences in a file

A test lives in a `.feature` file and looks like this:

```gherkin
Feature: Directory search
  Scenario: A known record is found
    Given the service is healthy
    When the operator searches for "Acme"
```

- **Feature** — what area you are testing. One per file.
- **Scenario** — one situation worth checking. A file can hold many.
- **Given / When / Then** — the steps, in order. proef strips the opening word
  and matches only the rest of the sentence, so all of them behave identically;
  pick whichever reads best. *Given* for setup, *When* for the action, *Then*
  for the check. **And** and **But** work too, and read better than repeating
  yourself:

```gherkin
    When the operator searches for "Acme"
    Then the response status is 200
    And the value at "$.results" is a non-empty list
```

Each step is one sentence, and every sentence must be one the vocabulary knows.
That is the only rule you have to satisfy.

## 2. See the sentences you may write

```console
$ proef macros
```

```
builtin:core.yaml
  expectPresent                0×  the value at {path} is present  (builtin, unused here)
  expectStatus                 0×  the response status is {status}  (builtin, unused here)
  …
suite/packs/api.yaml
  health                       1×  the service is healthy
  search                       1×  the operator searches for {term}

10 macro(s) · 0 unused
```

The right-hand column is what you can say. The left is the internal name — you
do not need it. The `1×` counts how many steps across the suite use that
sentence.

**`{term}` is a blank you fill in.** `the operator searches for {term}` means
you write:

```gherkin
    When the operator searches for "Acme"
```

Quotes are the house style and make the value easy to see.

This command works even when your file has a mistake in it — which is exactly
when you need it. If some step does not bind, you still get the list; only the
`1×` counts are withheld (they would be misleading), and they show as `—`.

> **Editor tip.** If someone has set up [`proef lsp`](EDITORS.md) for your
> editor, these sentences appear as autocomplete while you type, and mistakes
> underline as you go. It is worth asking for — but nothing on this page needs
> it.

## 3. Check before you run

```console
$ proef test --dry-run
```

This reads your file and tells you whether every sentence binds. It sends no
requests and touches nothing, so it is always safe and takes well under a
second. Run it constantly.

```
  ok suite/case.feature — 1 scenario(s), 2 step(s), 1 batch(es)

dry-run OK: 1 feature(s), 1 scenario(s), 2 step(s) …
next: proef test
```

When it says `next: proef test`, your sentences are good.

## 4. Run it

```console
$ proef test
```

```
  Scenario: A known record is found (suite/case.feature)
    ✓ suite/case.feature:3 — the service is healthy (6ms)
    ✗ suite/case.feature:4 — the operator searches for "Acme" (0ms)
```

`✓` passed · `✗` failed · `∅` skipped, because an earlier step in the same
scenario failed.

A `✗` here is a *real result* — proef reached the system and the answer was
wrong. That is the test doing its job. Take it to whoever owns the API, with
the line it names.

## 5. The two mistakes you will actually make

Everything else is someone else's problem. These two are yours.

### "no macro matches …"

```
proef::bind::unbound_step

  × no macro matches `the operator serches for "Acme"` — did you mean
    `the operator searches for {term}`?
   ╭─[suite/case.feature:4:5]
 4 │     When the operator serches for "Acme"
   ·     ────────────────────────────────────
   ╰────
```

You wrote a sentence the vocabulary does not know — usually a typo, a plural,
or a word order that drifted. proef points at the exact line and, when it can,
guesses what you meant.

**Your fix:** say a sentence that exists. `proef macros` lists them.

The message also shows a `macros:` block. That is instructions for the person
who maintains the vocabulary — hand it to them if the sentence you need
genuinely does not exist yet. It is not something you need to write.

### "url variable … is not set"

```
proef::resolve::missing_config_var

  × in macro `health`: url variable `bse` is not set — define `[url]` `bse`
    in proef.toml (or in the active `[env.<name>.url]`) — did you mean `base`?
```

A sentence you used needs an address that nobody has filled in. This one lives
in `proef.toml`, which is a short settings file, not a pack:

```toml
[url]
base = "https://api.your-company.com"
```

**Your fix:** if you recognise the name, correct the typo it suggests.
Otherwise this is a setup question for whoever configured the project.

### One more you may meet once

If a brand-new project fails on its very first run and says it is **still the
`proef init` scaffold**, nothing is broken — the starter files point at a
placeholder address and placeholder routes on purpose. Somebody needs to point
`[url] base` at the real API and replace the example routes. After that, the
loop above is all yours.

## 6. The loop

```
read the sentences   →   proef macros
write a scenario     →   your .feature file
check it binds       →   proef test --dry-run
fix what did not     →   the two errors above
run it               →   proef test
```

That is the whole job. When you need something the vocabulary cannot say, that
is a conversation with whoever maintains the packs — and
[AUTHORING.md](AUTHORING.md) is the page for them.

## Where to go next

| You want to | Read |
|---|---|
| Understand a symbol, exit code, or failure | [TROUBLESHOOTING.md](TROUBLESHOOTING.md) |
| Get autocomplete and live error underlining | [EDITORS.md](EDITORS.md) |
| Maintain the vocabulary yourself | [AUTHORING.md](AUTHORING.md) |
| Set up a project from scratch | [GETTING-STARTED.md](GETTING-STARTED.md) |
