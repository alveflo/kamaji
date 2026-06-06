# kamaji app icons

`icon.svg` is the master artwork (the "any" icons); `icon-maskable.svg` is the
full-bleed maskable / apple-touch variant; `favicon.svg` is the browser favicon.
The PNGs are rasterized from those SVGs.

## Regenerating the PNGs

Install the [resvg](https://github.com/linebender/resvg) CLI once:

```sh
cargo install resvg
```

Then rasterize (run from this directory). `-w` / `--width` set the output width
in pixels:

```sh
resvg --width 192 icon.svg          icon-192.png
resvg --width 512 icon.svg          icon-512.png
resvg --width 512 icon-maskable.svg icon-maskable-512.png
resvg --width 180 icon-maskable.svg apple-touch-icon.png
```

Run `resvg --help` to see all options (flag names can vary by resvg version).
