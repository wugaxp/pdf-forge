<p align="center">
  <img src="docs/brand/pdf-forge-wordmark.png" alt="PDF Forge — Open Source PDF Pro" width="420">
</p>

<h1 align="center">PDF Forge</h1>

<p align="center">
  <b>PDF Forge</b> — a personal, security- and performance-hardened fork of
  <a href="https://github.com/storytold/pdfcraft">PdfCraft</a>.
</p>

> **Fork notice**
>
> PDF Forge is a personal fork of
> [storytold/pdfcraft](https://github.com/storytold/pdfcraft), based on upstream **v0.14.0**,
> still upstream's newest tag (no `v0.14.1` exists there). It is **not affiliated with or
> endorsed by the ArtCraft team**. The ArtCraft name, wordmark and logo files have been removed;
> this fork is distributed under the upstream project's MIT OR Apache-2.0 license.

## What PDF Forge changes

The genuine fork delta: everything below differs from upstream v0.14.0, the range
`git diff v0.14.0..HEAD`. Behaviour upstream already shipped is listed separately under
[Inherited from upstream](#inherited-from-upstream-v0140) and is not claimed here.

### Safety

- **Undo cannot restore removed content.** Redaction, hidden-information removal and encryption
  changes clear the undo/redo stacks when saved, so **Undo + Save cannot write removed content
  back** into the file.
- **Certification signatures are a write barrier.** DocMDP P=1 refuses edits, signing, timestamps
  and LTV updates; P=2 allows form filling; P=3 also allows annotations and links.
- **R5/R6 encryption permissions fail closed.** A missing or damaged authenticated `/Perms`
  block grants no user permissions instead of trusting the editable `/P` (upstream already
  intersected the two; the fork no longer falls back to `/P`).
- **Document JavaScript is bounded in time and space.** A 5-second deadline, a Boa operand-stack
  bound, a smaller worker stack, and a cap on abandoned script workers. (Upstream already bounds
  Boa's loop iterations and recursion depth.)

### Memory and hostile input

- **Bounded extraction and caches.** Glyph extraction is capped at 1M glyphs per page, the
  soft-mask and outline caches at 256 and 4096 entries, and decoded streams default to 512 MiB
  with explicit budgets at the prune, preflight, redaction-interpretation and form-detection
  call sites.
- **Pixel allocation is fallible** instead of aborting when the allocator refuses.
- **Hostile structures are bounded or invalidated.** Named destinations are capped (100,000
  entries, 16 MiB), object-stream caches are dropped when an object is replaced, and object-scan
  misses are cached so a hostile xref cannot cost a rescan per object.

### Performance

- **Page-tree walks are linear.** Page creation, redaction, preflight, sanitize, OCR and print
  text-direction scans build page/annotation indexes once instead of rebuilding the page tree
  per page or per annotation.
- **The render pool survives edits.** `RenderPool` replaces its document bytes without recreating
  worker threads and parser state, and the canvas re-submits its current demand after a
  replacement.
- **Cheaper refreshes.** Comment and content refreshes skip needless display/form rewrites, and
  form fields are not enumerated on every comment edit.
- **Long work stays off the UI thread.** Tiled pages reuse a bounded whole-page surface per
  worker (pages up to 4 MP); Compare and large redaction searches run off the UI thread; search
  text is bounded by a scanned-page set; and Find updates insert one page's matches instead of
  re-sorting the whole result list.

### Chinese text

- **Simplified-Chinese text works.** Chinese Add Text shapes Han characters and falls back to a
  Noto Sans CJK SC Type 3 face, with a clear error when the build has no CJK face; Chinese lines
  may break between characters.
- Builds may set `CRAFT_FONTS_DIR` to a `craft-fonts` checkout for the optional Japanese/CJK
  faces — an upstream build input, see below; builds without it still compile and run.

### Product identity

- **PDF Forge is the user-facing brand.** The window title, the About dialog and the home screen
  say PDF Forge, the Help links point at [wugaxp/pdf-forge](https://github.com/wugaxp/pdf-forge)
  and at upstream, the ArtCraft community links are gone, and `docs/brand/` holds this fork's own
  icon, mark and wordmark (trademark terms in `docs/brand/LICENSE-brand.txt`).

## Inherited from upstream v0.14.0

Unchanged from upstream — not this fork's work, and not claimed above:

- Page rasters are capped at 64 MP (256 MB RGBA) and 8192 px per side.
- Boa's script loop-iteration and recursion limits.
- The decode-budget mechanism (`DecodeBudget`, `decoded_within`); the fork lowers the default
  and adds budgets at more call sites.
- CJK and Japanese interface fallback, the Japanese text faces, and the optional
  `CRAFT_FONTS_DIR` build input that `cargo build` copes without.
- The `pdfcraft`, `pdfcraft-cli` and `pdfcraft-*` crate and binary names, and the whole PDF
  engine, renderer, forms and signing stack upstream built.
- The app icon in `assets/app-icon/` (upstream's artwork, MIT OR Apache-2.0) and the reference
  screenshots in `docs/images/` (generated upstream with `cargo xtask screenshots`).

## Known limits

- JavaScript still has no hard heap cap; the deadline and runtime limits bound time and stack
  use, but a single very large JS allocation can still exhaust memory. Process isolation or an
  OS-level memory limit would be required for a hard guarantee.
- Raster memory is bounded per request/worker/cache, not by one process-wide budget.
- The native application and crates keep their original technical names
  (`pdfcraft`, `pdfcraft-cli`, `pdfcraft-*`) so the fork stays build-compatible; **PDF Forge**
  is the fork's user-facing brand.

## Build

The project is a Rust workspace:

```sh
cargo build --release
cargo test
cargo run -p pdfcraft
```

Optional CJK fonts:

```sh
CRAFT_FONTS_DIR=/path/to/craft-fonts cargo build --release
```

The CLI is `target/release/pdfcraft-cli`. The desktop app is `target/release/pdfcraft`.

## Branch and upstream

This is **PDF Forge 0.14.1**, based on upstream **v0.14.0**. Upstream has published no
`v0.14.1` tag (its newest is still `v0.14.0`); the fork's workspace version records its own
next version.

To follow upstream:

```sh
git remote add upstream https://github.com/storytold/pdfcraft.git
git fetch upstream --tags
```

The fork is maintained independently; changes are not automatically proposed upstream.

## License and attribution

PDF Forge is derived from PdfCraft, Copyright (c) 2026 ArtCraft Team and the PdfCraft
contributors, and is licensed under **MIT OR Apache-2.0** at your option. See `LICENSE-MIT`,
`LICENSE-APACHE`, `NOTICE`, `ATTRIBUTION.toml` and `ATTRIBUTION.md` for the full notices: they
separate what this fork adds from the assets inherited from upstream, whose app icon and
reference screenshots are credited there.

The ArtCraft marks are **not** included in this fork. No ArtCraft logo, wordmark
or brand asset is distributed here; the upstream ArtCraft files in `docs/brand/` have been
replaced with the fork's own `pdf-forge` artwork, whose name, wordmark and mark belong to wugaxp
(`docs/brand/LICENSE-brand.txt`).