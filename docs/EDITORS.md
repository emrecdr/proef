# Editor setup — `proef lsp`

`proef lsp` starts a [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
server that speaks **generic LSP over stdio**. Point any LSP-capable editor at
`proef lsp` and get, live as you type:

- **Diagnostics** — the same validation `proef test --dry-run` performs (unbound
  steps, unknown step kinds, malformed hurl blocks, unresolved `${…}`
  references), published across the whole suite as you edit.
- **Go-to-definition** — jump from a Gherkin step to the macro that binds it, or
  from a pack's `ref:` to the `# @proef` annotation in the `.hurl` file
  (ADR-0018).
- **Completion** — step completions offering the suite's macro patterns; on a
  pack's `ref:` line, the fragment names in scope; and inside a `bind:` table,
  the `{{variables}}` those fragments actually read, each labelled with the
  fragment that wants it. A variable a fragment supplies itself
  (`[Options] variable:`) is left out: it needs no `bind:`, and binding it would
  be refused as `option_declared_twice`.
- **Find-references** — every step across the suite that a given macro binds.
- **Document symbols** — a feature outlines to its scenarios (tagged ones
  showing their tags), a pack to its macros (detailed by the pattern each
  matches). Which vocabulary applies is decided by what discovery found in the
  file, not by its extension.
- **Hover** — the macro a step binds, or a `use:` targets, with its pack,
  pattern and params; on a `ref:`, the fragment's file and the variables still
  needing a `bind:`. Every fact comes from the same analysis the diagnostics
  do, so a hover can never contradict the squiggle on the same line.
- **Semantic tokens** — the two variable tiers, told apart on screen. Inside a
  pack's `hurl: |` block, `${…}` (resolved at **lower time**, by proef, before
  any request exists) and `{{…}}` (resolved at **run time**, by hurl) look
  identical to every editor: a YAML highlighter sees a string, and a hurl
  highlighter never runs because the block is not a file. proef is the only
  party that knows which is which. `${…}` is reported as a **macro** — a
  substitution performed before execution, which is what a macro is — and
  `{{…}}` as a **variable**; every mainstream theme colours those differently
  already, so nothing needs configuring. A `$${` escape stays unhighlighted,
  because it is a literal `${` proef will not substitute.
- **Quick fixes** — a misspelled name that already earned a "did you mean"
  becomes an applicable edit: `use:` and `ref:` targets, `with:` and `bind:`
  keys, step kinds, Examples placeholders, and data-table columns. A fix is
  offered only when it is certain — the suggested name is near enough, and the
  misspelling occurs exactly once as a whole token in that same file — so
  applying one is never a guess. Reach it from the squiggle or from the token
  itself; a `use:` error underlines the macro's name key, which is often several
  lines above the word you typed.

The server analyzes **the configured suite** — `proef.toml`'s `[run] suite` if
set, else the `tests/` convention — resolved under the directory it is launched
in (its working directory), discovering every `.feature` file and every
`packs/*.yaml` / `packs/*.yml` macro pack beneath that root — the same
resolution `proef test` uses, so the two never diverge. Launch your editor from
the project root (or configure the server's root/working directory to it).

## When analysis fails

A panic inside analysis or a feature never ends the server. proef reports it
once through `window/showMessage` — the channel an editor actually surfaces —
and keeps serving; the next edit retries, and reports again only if the new
state also fails. A server that died would show *nothing*, which reads as "proef
has no opinion about this file" rather than as the failure it is.

Running `proef lsp` by hand also prints the panic to stderr, which is where the
detail lives.

## Naming the config: `--config`

`proef lsp --config <path/to/proef.toml>` names the config to read instead of
searching for one, exactly as it does for every other subcommand (`CONFIG.md`).
Reach for it when discovery cannot find the right file — most often a config
that sits *beside the suite* rather than above it, which an upward search
launched from the repository root can never reach.

For `proef lsp` the flag also outranks the workspace root the client announces:
the flag names a file, and a named file is not a guess to be improved on.
Without it, an editor rooted somewhere else loads a different config than the
runner, and the diagnostics stop being trustworthy in exactly the layout the
flag exists for — `proef test --config …` runs green while every `ref:` reads as
unknown in the editor.

```lua
cmd = { "proef", "lsp", "--config", "/abs/path/to/proef.toml" },
```

Unlike the runner, an unreadable or absent file does not stop the server: it
starts on defaults, because an editor offering less is better than one that will
not boot. A **relative** path works, but prefer an absolute one — an editor's
working directory is rarely the one you assume.

## File types served

| Kind | Pattern | Typical editor filetype |
|---|---|---|
| Feature files | `*.feature` | `gherkin` / `cucumber` / `feature` |
| Macro packs | `packs/*.yaml`, `packs/*.yml` | `yaml` |

## Neovim

Built-in LSP client (Neovim 0.8+), no plugin required. Add to your config and
open a `.feature` file from inside the suite:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = { "cucumber", "gherkin", "yaml" },
  callback = function(args)
    vim.lsp.start({
      name = "proef",
      cmd = { "proef", "lsp" },
      -- The server scopes analysis to the configured suite under its launch
      -- directory; anchor root_dir at the nearest proef.toml (or the current
      -- file's directory) so the two agree.
      root_dir = vim.fs.dirname(
        vim.fs.find({ "proef.toml" }, { upward = true, path = args.file })[1]
      ) or vim.fs.dirname(args.file),
    })
  end,
})
```

With [`nvim-lspconfig`](https://github.com/neovim/nvim-lspconfig) you can instead
register it as a custom server via `vim.lsp.config`/`configs`, using the same
`cmd = { "proef", "lsp" }`.

## Helix

Add a server and attach it to the languages you author. In
`~/.config/helix/languages.toml`:

```toml
[language-server.proef]
command = "proef"
args = ["lsp"]

[[language]]
name = "gherkin"
language-servers = ["proef"]

# Attach to pack YAML too, so pack diagnostics surface while editing macros.
[[language]]
name = "yaml"
language-servers = ["proef", "yaml-language-server"]
```

Run `hx --health gherkin` to confirm Helix found the `proef` binary.

## Emacs (Eglot)

Eglot ships with Emacs 29+. Associate your feature-file major mode (for example
[`feature-mode`](https://github.com/michaelklishin/cucumber.el)) with the
server:

```elisp
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '(feature-mode . ("proef" "lsp"))))
;; M-x eglot in a .feature buffer opened from the suite root.
```

For `lsp-mode`, register a stdio client whose connection is
`(lsp-stdio-connection '("proef" "lsp"))` for the same major mode.

## v1 limitations

This is the first release of the language server. Known boundaries:

- **`proef.toml` config is a startup snapshot.** `${url:…}` / `${vars:…}` values
  are read once when the server starts. After editing `proef.toml` (or switching
  `PROEF_ENV`), **restart the server** to pick up the change; until then those
  references analyze against the old (or, if the file could not be loaded at
  startup, an empty) scope and may warn.
- **Built-in macros have no jump target.** The `expect*` family lives in a pack
  compiled into the binary, not a file on disk, so there is nothing for
  go-to-definition to open. Hover still answers — the macro is in the analysis
  like any other, and reports its pack as `builtin:…`, which is *why* the jump
  is unavailable. `proef macros` lists the whole family with the sentence each
  binds.
- **Completion ranking is best-effort.** All of the suite's macros are offered;
  ranking is a lightweight edit-distance heuristic. Full context-aware ranking is
  a follow-up.
- **External edits to closed files need a reopen.** The server does not watch the
  filesystem; it re-reads a file's bytes from its open editor buffer. If a
  `.feature` or pack file is changed outside the editor (or by another tool) while
  closed, reopen it so the server sees the new bytes.
- **No VS Code extension yet.** v1 is a server-only generic-LSP binary. It works
  with any editor that speaks generic LSP (Neovim, Helix, Emacs, Sublime LSP, …);
  a VS Code wrapper is a possible follow-up.

  If one is built, its `documentSelector` must match on **path**
  (`{ scheme: "file", pattern: "**/*.feature" }`), not on a language id. The
  ecosystem is split — the two established Gherkin extensions register
  `cucumber` and `feature` respectively — so a selector naming either id
  attaches for some users and silently does nothing for the rest. The table
  under [File types served](#file-types-served) is that split; a path selector
  is the only thing all of it has in common.

- **No Zed support.** Zed binds language servers to languages it has a
  tree-sitter grammar for, and there is no Gherkin grammar in it. That grammar
  is a prerequisite, not a configuration step, so Zed waits on work outside this
  repository.
- **Overlay lookup can still miss if the suite root is reached through a
  symlink.** The root is deliberately left uncanonicalized (canonicalizing
  would resolve symlinks and desync source names from the client's document
  URIs), so if an editor resolves a symlinked suite root differently than the
  server's raw working directory, the overlay lookup can miss and the LSP
  analyzes the saved on-disk bytes instead of the unsaved buffer. An editor's
  percent-encoding choice no longer matters here — the overlay matches open
  buffers by decoded source name, not the raw URI.
