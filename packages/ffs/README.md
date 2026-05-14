# @ffs-cli/ffs

ffs — a typo-resistant, frecency-ranked file & code search CLI. One binary
that replaces `find`, `fd`, `grep`, `rg`, `glob`, and `cat`, plus tree-sitter
powered code-navigation (`symbol`, `callers`, `callees`, `refs`, `flow`,
`impact`) and a token-budget aware file reader for AI agents.

See the main project README for full documentation:
https://github.com/dmtrKovalenko/ffs.nvim

## Install

```bash
npm install -g @ffs-cli/ffs
# or
pnpm add -g @ffs-cli/ffs
# or
yarn global add @ffs-cli/ffs
```

The postinstall script downloads the appropriate native binary for your
platform from the project's GitHub releases.

Supported platforms:
- Linux x64 / arm64 (musl-linked, works across glibc versions)
- macOS x64 / arm64
- Windows x64 / arm64

## Use

```bash
ffs --help
ffs index                                 # one-time warm-up
ffs find <query>
ffs grep <pattern>
ffs symbol <name>
ffs read <path> --budget 5000 --filter minimal
ffs dispatch '<free-form query>'
```

## Skip the postinstall download

```bash
FFS_SKIP_POSTINSTALL=1 npm install -g @ffs-cli/ffs
```

Then drop a `ffs` (or `ffs.exe` on Windows) binary into the package's `bin/`
directory yourself.

## Build from source

```bash
git clone https://github.com/dmtrKovalenko/ffs.nvim.git
cd ffs.nvim
cargo build --release -p ffs-cli --features zlob
# binary at target/release/ffs
```

## License

MIT
