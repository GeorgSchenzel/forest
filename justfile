default:
    @just --list

# Build and install forest into ~/.cargo/bin
install:
    cargo install --path . --locked --force

# Remove forest from ~/.cargo/bin
uninstall:
    cargo uninstall forest
