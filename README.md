# Umamusume DarkMode

Umamusume DarkMode is a Hachimi plugin that makes Umamusume: Pretty Derby's UI dark.

Instead of laying a tint over the screen, it recolours the game's own UI textures (buttons, panels,
backgrounds, list rows) as they load, keeps character art untouched, and adjusts text and outline
colours so everything stays readable.

## Installation

1. Install Hachimi

Download and install Hachimi Edge from:

`https://github.com/kairusds/Hachimi-Edge/releases/latest`

2. Download `hachimi_darkmode.dll`

Grab the latest `hachimi_darkmode.dll` from:

`https://github.com/cBachoo/hdm/releases/latest`

Place it in your game folder root (next to `UmamusumePrettyDerby.exe`).

3. Enable the plugin in Hachimi

Open `hachimi/config.json` in your game folder and add `hachimi_darkmode.dll` to `load_libraries`:

```json
{
  "load_libraries": [
    "hachimi_darkmode.dll"
  ]
}
```

4. Launch the game

The first launch takes a little longer while the UI textures are converted; the results are cached
in `hachimi/darkmode/cache/`, so later launches are quick.

## What it changes

- Buttons, panels, dialogs, list rows, tabs and backgrounds
- Home, Story, Race and event menu cards (the artwork is kept, the white label panels go dark)
- Text and outline colours, so labels stay readable on the dark UI
- Screen transition and loading screen backdrops

## In-game menu

Open the Hachimi menu and find the **Dark Mode** section:

- **Enabled**: turn dark mode on/off (applies to textures loaded afterwards)
- **Light text**: remap dark text colours to light ones
- **Reload overrides.json & re-apply**: reload custom rules (see below)

## Configuration

After first launch, `hachimi/darkmode/config.json` will contain:

```json
{
  "enabled": true,
  "remap_text": true
}
```

### `enabled`

If `false`, the plugin leaves the game's look untouched.

### `remap_text`

If `true`, dark text colours are turned light (and their outlines dark) so text is readable on the
dark UI.

## Custom rules (optional)

The plugin ships with built-in rules for which textures get darkened and how. To customise them,
put your own `overrides.json` in `hachimi/darkmode/`; it replaces the built-in rules. Start from
[`tools/overrides.json`](https://github.com/cBachoo/hdm/blob/main/tools/overrides.json); the format is described in
[DEVELOPMENT.md](DEVELOPMENT.md).

## Uninstalling

Remove `hachimi_darkmode.dll` from `load_libraries` (or delete the DLL), and optionally delete the
`hachimi/darkmode/` folder.
