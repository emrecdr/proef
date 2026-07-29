# Upstream briefing #2 — hurl cannot be built on Windows from crates.io

**Status:** patch prepared and verified; branch pushed to the fork. The issue and PR
text must be written by a human in their own words (hurl's contribution policy), and
the commit must be signed before opening a PR — this document is the factual briefing
to write them from, not text to paste.

- Fork branch: `emrecdr/hurl` → `crates-io-windows-icon` (one commit,
  `Skip Windows icon embedding when logo.ico is absent`; currently unsigned —
  amend + sign before opening a PR)
- Patch file: `0002-skip-windows-icon-when-absent.patch` (applies clean on the
  `8.0.1` tag **and** current master)
- Relationship to briefing #1 (`0001-*`): independent — different file, no overlap;
  the two PRs can proceed in any order.

## The defect

`packages/hurl/build.rs` embeds a Windows resource icon:

```rust
#[cfg(windows)]
fn set_icon() {
    let mut res = WindowsResource::new();
    res.set_icon("../../bin/windows/logo.ico");
    res.compile().unwrap();
}
```

`bin/windows/logo.ico` lives at the **repository root**, two directories above the
`packages/hurl` package directory. `cargo package` can only include files under the
package directory, so the icon is not in the published crate — and cannot be, at that
path. Every Windows (MSVC) build of `hurl` as a crates.io dependency therefore fails
in the build script:

```
error: failed to run custom build command for `hurl v8.0.1`
  ...\out\resource.rc(25) : error RC2135 : file not found: ../../bin/windows/logo.ico
  thread 'main' panicked at hurl-8.0.1\build.rs:29:19:
  called `Result::unwrap()` on an `Err` value: Custom { kind: Other,
      error: "Could not compile resource file" }
```

Verified affected: 8.0.1 (observed in our CI on `windows-latest`, MSVC). The same
code is present on master (`03fcb84`), so the next release inherits it. Hurl's own
Windows release builds never see this — they build inside the repository where the
relative path resolves — which is presumably why it has gone unnoticed. Anyone using
hurl as a library on Windows hits it unconditionally.

Reproduction (any Windows host):

```powershell
cargo new icon-repro && cd icon-repro
cargo add hurl@8.0.1     # plus the vcpkg-provided libcurl/libxml2 the crate needs
cargo build              # fails with RC2135 before any linking happens
```

## The fix in the patch (option A — minimal)

Embed the icon only when the file exists; warn otherwise:

```rust
let icon = "../../bin/windows/logo.ico";
if !Path::new(icon).exists() {
    println!("cargo:warning=logo.ico not found (building outside the Hurl \
              repository?); the binary is built without an embedded icon");
    return;
}
```

In-repository builds (including hurl's release pipeline) are byte-for-byte unchanged —
the file exists there, so the same `WindowsResource` path runs. Only the
previously-impossible case (crate consumers) changes: they now build, without an
embedded icon they were never going to get anyway.

Verification performed:
- patch applies clean on the `8.0.1` tag and master (`git apply --check`)
- `cargo check -p hurl` green on the patched master checkout (unix arm)
- the windows arm type-checks as written against `winres 0.1.12` (compiled with the
  `cfg` gate removed on a non-windows host — same imports, same API calls)
- not run: an actual Windows build of the patched crate (our CI proves the
  *workaround* below on `windows-latest`; the patched build script's guard is the
  same `Path::exists` the workaround satisfies)

## Alternative the maintainers may prefer (option B)

Move `logo.ico` under `packages/hurl/` (e.g. `packages/hurl/windows/logo.ico`) and
shorten the relative path — then the icon ships in the crate and consumers get the
embedded icon too. More correct, but it relocates a repo asset that the NSIS/choco
packaging scripts under `bin/windows/` may also reference; that blast radius is why
the prepared patch takes option A. Worth mentioning both in the issue and letting
maintainers choose.

## Downstream workaround (what proef's CI does meanwhile)

Supply the file at the paths the resource compiler can resolve, before `cargo build`
(see `.github/workflows/windows.yml`): download `bin/windows/logo.ico` from the
pinned tag and copy it to `<cargo registry src root>/bin/windows/logo.ico` and
`target/debug/build/bin/windows/logo.ico`. Ugly but hermetic to CI; remove once an
upstream release contains the fix.
