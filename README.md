<!--
SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>

SPDX-License-Identifier: CC-BY-NC-SA-4.0
-->

# photon-change

`photon-change` fixes incorrect image file extensions by inspecting each file's contents.

For example, if `picture.jpg` actually contains PNG data, it is renamed to `picture.png`.

## Features

- Detects image type from file content (not filename)
- Supports `--dry-run` to preview changes
- Traverses directories recursively by default

## Install / Build

```bash
cargo build --release
# or
cargo install --path .
```

## Usage

See `photon-change --help` for full usage instructions.
