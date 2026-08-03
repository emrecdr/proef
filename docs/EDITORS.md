# Editor setup — `proef lsp`

`proef lsp` starts a [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
server that speaks **generic LSP over stdio**. Point any LSP-capable editor at
`proef lsp` and get, live as you type:

- **Diagnostics** — the same validation `proef test --dry-run` performs (unbound
  steps, unknown step kinds, malformed hurl blocks, unresolved `${…}`
  references), published across the whole suite as you edit.
- **Go-to-definition** — jump from a Gherkin step to the macro that binds it.
- **Completion** — step completions offering the suite's macro patterns.
- **Find-references** — every step across the suite that a given macro binds.

The server analyzes **one suite rooted at the directory it is launched in**
(its working directory), discovering every `.feature` file and every
`packs/*.yaml` / `packs/*.yml` macro pack beneath it — the same discovery a
normal `proef` run uses. Launch your editor from the suite root (or configure the
server's root/working directory to it).

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
      -- The server analyzes the suite rooted at its working directory; anchor
      -- it at the nearest proef.toml (or the current file's directory).
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
- **Go-to-definition lands on the macro name.** The jump targets the macro's name
  key in its pack, not a specific `match:` line (macros bind by pattern, so there
  is no single match span to point at).
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
