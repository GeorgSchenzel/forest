# Install forest into cargo's bin dir (~/.cargo/bin)
install:
    cargo install --path . --locked

# Remove the installed forest binary
uninstall:
    cargo uninstall forest
