# Installing proef

Pick whichever fits your machine:

```bash
# Homebrew (macOS or Linuxbrew, arm64 or x86_64):
brew install emrecdr/proef/proef

# Prebuilt binary via cargo-binstall (any Rust dev environment):
cargo binstall proef

# From source via crates.io:
cargo install proef --locked
```

Or grab a prebuilt archive from a
[GitHub Release](https://github.com/emrecdr/proef/releases/latest) — five
targets ship per tag (macOS arm64/x86_64, Linux arm64/x86_64-gnu, Windows
x86_64-msvc), each with SLSA build provenance
(`gh attestation verify <archive> --owner emrecdr`) and a `.sha256` sidecar
(`sha256sum -c proef-<tag>-<target>.tar.gz.sha256` from the download
directory). The Windows zip bundles the libcurl/libxml2 DLLs the binary
needs; Linux binaries expect the distro's `libcurl4` and `libxml2` (present
on virtually every system, or `apt install libcurl4 libxml2`).

Prebuilt binaries need no Rust toolchain and no build prerequisites.
Building from source (the `cargo install`/`cargo binstall`-fallback path)
needs the native libraries the embedded hurl engine links:
`apt install build-essential pkg-config libssl-dev libcurl4-openssl-dev
libxml2-dev libclang-dev` on Linux; the Xcode command-line tools suffice on
macOS.

## Completions and the man page

Shell completions and a man page travel in every release archive
(`completions/`, `proef.1`) — or generate them from any installed binary:

```bash
proef completions zsh > "${fpath[1]}/_proef"   # also bash, fish, powershell, elvish
proef man > /usr/local/share/man/man1/proef.1
```

## In CI

`cargo binstall proef` resolves the release archives, so any workflow with a
Rust toolchain installs in seconds; without one, download an archive and its
`.sha256` from the Release directly. A complete GitHub Actions workflow —
secrets, JUnit, sharding, the regression gate — is in [CI](CI.md).

## First run

`proef doctor` verifies the environment (embedded engine, native libraries,
project layout, runs-dir writability). Then [Getting started](GETTING-STARTED.md)
takes you from `proef init` to a green run.

## A note on the dev fixture

The tutorial's zero-network first run uses the dev fixture API
(`cargo run -p xtask -- fixture`), which lives in this repository and is not
part of the installed binary — it needs a checkout and a Rust toolchain. With
an installed binary alone, point `[url] base` at any API you own instead; the
`proef init` scaffold passes as-is against the fixture, and its routes
(`/health`, `/search`, `/version`) are one file away from targeting yours.
