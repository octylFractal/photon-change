<!--
SPDX-FileCopyrightText: Octavia Togami <octy@octyl.net>

SPDX-License-Identifier: CC-BY-NC-SA-4.0
-->

# photon-change

`photon-change` applies various fixes to photos.

## Features

- Can fix incorrect image file extensions by inspecting each file's contents.
  - Detects image type from file content
- Can apply the `photoTakenTime` from Google Photos json metadata files to the _file_ modification time.
  - This is primarily good for forcing the Memories Nextcloud add-on to sort photos properly.
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
