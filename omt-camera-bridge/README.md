# omt-camera-bridge

Reads raw YUV420 (I420) camera frames from the `c2` Python app over a local
Unix socket, converts them to UYVY, and streams them out over
[Open Media Transport](https://www.openmediatransport.org/) (OMT) so the
camera shows up as a source in OBS, vMix, etc.

See `../c2/socketbridge.py` for the Python side that feeds this process.

## Quick start

You can use a plain Rust toolchain (`rustup`) but [Nix](https://nixos.org/download/#download-nix) makes things arguably easier.


### Build

```bash
nix build
./result/bin/omt-camera-bridge
```

`nix build` always builds for your current machine's platform

### Dev shell

```bash
nix develop
```

this should set up everything you need for compiling the project.

```bash
# Build for the machine you're on right now - fast, uses your native linker
cargo build --release

# Cross-compile - use `cargo zigbuild` (not `cargo build`) for any target
# other than your host's. Zig acts as the C linker/toolchain for every
# target below, on any of the 4 host platforms, so no per-target system
# toolchain installs are needed.
cargo zigbuild --release --target aarch64-unknown-linux-musl   # Raspberry Pi (64-bit)
cargo zigbuild --release --target x86_64-unknown-linux-musl    # generic Linux x86_64
cargo zigbuild --release --target aarch64-apple-darwin         # Apple Silicon Mac
cargo zigbuild --release --target x86_64-apple-darwin          # Intel Mac
```

Cross-compiled binaries land under
`target/<target-triple>/release/omt-camera-bridge` as usual for Cargo.

We use musl to produce fully static binaries with zero runtime dependencies.

### Deploying to the Raspberry Pi

The Pi runs `aarch64-unknown-linux-musl`. From your dev machine:

```bash
cargo zigbuild --release --target aarch64-unknown-linux-musl
scp target/aarch64-unknown-linux-musl/release/omt-camera-bridge root@<pi-ip>:/opt/omt-camera-bridge/omt-camera-bridge
ssh root@<pi-ip> 'chmod +x /opt/omt-camera-bridge/omt-camera-bridge'
```

## Running it

```
omt-camera-bridge [--socket PATH] [--name NAME] [--width W] [--height H] [--fps N]
```

| Flag | Default | Meaning |
|---|---|---|
| `--socket` | `/run/c2-video.sock` | Unix socket the Python app is serving raw I420 frames on |
| `--name` | `C2 Camera` | OMT source name - shows up as `<hostname> (<name>)` in receivers |
| `--width` | `1920` | Must match the Python app's "main" stream width (must be even) |
| `--height` | `1080` | Must match the Python app's "main" stream height |
| `--fps` | `30` | Advertised frame rate (metadata only - actual rate follows whatever Python sends) |
