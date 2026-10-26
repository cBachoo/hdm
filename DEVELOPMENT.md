# Development

## Layout

```
darkmode/          Rust plugin (cdylib -> hachimi_darkmode.dll)
  src/
    lib.rs         entry point (hachimi_init_v3), Hachimi menu
    textures.rs    hooks on texture/atlas loading, applies replacements
    runtime.rs     decides per texture what to do (from the rules), GPU readback, cache
    process.rs     image processing (darken, white swap, card compositing, ...)
    rules.rs       rules loading (built-in or custom overrides.json) and name matching
    text.rs        text fill / gradient / outline colour remapping
    tick.rs        per-frame work: loose textures, white overlays, cameras, outlines
    unity.rs       IL2CPP / Unity bindings
    dump.rs        dev build only: method dump for finding hook targets
  build.sh         builds dist/release and dist/dev
tools/
  overrides.json   the rules (embedded into the DLL at build time)
  manual/          optional hand-made patches for previewing
  build_dark_atlases.py   reference implementation of the processing, for previewing/tuning
  review.py        before/after review sheets per atlas
research/          local only, never committed (game assets); see "Setting up research/"
  dump/            original atlases/textures (dev build, "Dump original textures")
  loaded_assets.txt sprite rects per atlas (dev build, "Log loaded textures")
  out/             Python-built reference output (parity tests compare against it)
  methods_*.txt    class/method lists of the game assemblies (for finding hook targets)
```

## Setting up `research/`

`research/` holds textures dumped from your own copy of the game, so it isn't in the repository.
The plugin and its build don't need it; it's only used by the Python preview tools and the parity
tests (which skip themselves when it's missing). To create it:

1. Build and install the dev build (`cargo build --release --features dev`).
2. In the game, open the Hachimi menu → Dark Mode and turn on **Log loaded textures** and **Dump
   original textures**. Textures are dumped before they're darkened, so these are always the
   originals.
3. Visit the screens you want to work on (atlases only load when a screen uses them), then press
   **Rescan loaded atlases** to catch the ones loaded at boot.
4. Copy the results from the game into the project:
   ```
   <game>/hachimi/darkmode/dump/              -> research/dump/
   <game>/hachimi/darkmode/loaded_assets.txt  -> research/loaded_assets.txt
   ```
5. Build the Python reference output used by the parity tests:
   ```
   pip install numpy pillow scipy
   python tools/build_dark_atlases.py research research/out
   ```
6. Optional, for finding new hook targets: set `"dump_methods": true` in
   `<game>/hachimi/darkmode/config.json`, start the game once, and copy the `methods_*.txt` files it
   writes to `research/`.

Turn the dump options off again afterwards; dumping slows down loading.

## Building

```
cd darkmode
cargo build --release                  # user build: simple menu, no diagnostics
cargo build --release --features dev   # dev build: logging, dumps, debug tools
./build.sh                             # both, into dist/release and dist/dev
cargo test --release                   # Rust vs Python parity tests (skipped without research/)
```

The dev build adds to the Hachimi menu: "Log loaded textures", "Dump original textures", "Rescan
loaded atlases", "Dump UI sprite usage", and honours these extra `config.json` fields:
`log_assets`, `dump_textures`, `dump_methods` (dumps all classes/methods once on next start), and
`debug_hide` (GameObject path substrings to deactivate, for finding what draws something).

## Releasing

GitHub Actions builds everything on Windows (`.github/workflows/`):

- `ci.yml`: every push and pull request builds the plugin and runs the tests.
- `release.yml`: publishes a GitHub Release for the version in `darkmode/Cargo.toml`, tagged
  `v<version>`, with `hachimi_darkmode.dll` attached (the user build; build the dev variant locally
  when you need it).

To release, bump `version` in `darkmode/Cargo.toml` and push to `main`; the workflow runs when that
file changes. It can also be started manually from the Actions tab. If the tag for the current
version already exists it does nothing, so a version is never published twice.

The rules in `tools/overrides.json` are embedded at build time, so rule changes ship with the next
release.

## How textures are handled

For every texture/atlas sprite the game loads, the first of these that applies wins:

1. A full replacement PNG: `hachimi/darkmode/{atlas,textures}/<name>.png` or
   `hachimi/darkmode/manual/textures/<name>.png`.
2. A hand-made sprite patch: `hachimi/darkmode/manual/<Atlas>/<sprite>.png`.
3. A rule (exact name, `Atlas/name`, or `*` glob; first match in file order):
   - `"level"`: `background` / `panel` / `surface` / `raised` or `[r, g, b, scale]`
   - `"skip": true`
   - `"mode": "white_swap"` (pastel gradients -> deep versions of themselves)
   - `"mode": "card"` (illustrated cards: dark base + original artwork; options `direction`,
     `panel_start`, `blend`, `ring`, `feather`, `protect_outlined`, `protect_art`)
   - `"light_glyphs": true` (turn dark glyphs on the sprite light, e.g. info buttons)
   - textures: `"mode": "regions"` (large white areas only)
4. The `fallback` rules for anything not reviewed yet (e.g. new event assets): base UI piece names
   get darkened unless they look like artwork; illustrated cards get the card treatment.

Processed results are cached in `hachimi/darkmode/cache/`, keyed by texture name/size, the rules,
the game version and `PROCESS_VERSION` in `runtime.rs` (bump it when processing output changes).

## Tuning workflow

1. Make sure `research/` has dumps of the screens you're tuning (see "Setting up research/").
2. Use the dev build's **Dump UI sprite usage** on a screen to find which sprite/texture an element
   uses (`<game>/hachimi/darkmode/ui_usage.txt`).
3. Edit `tools/overrides.json`, then preview:
   ```
   python tools/build_dark_atlases.py research research/out
   python tools/review.py research research/out research/review
   ```
4. Copy `tools/overrides.json` to `hachimi/darkmode/` and press "Reload overrides.json & re-apply"
   to try it in game, then rebuild the DLL to embed it (and delete the copy from the game folder).
