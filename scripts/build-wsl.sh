#!/bin/bash
# Build script for Exo on Windows via WSL2
# This script builds the Linux exo binary inside WSL2

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== Exo WSL2 Build Script ===${NC}"

# Detect if we're running in WSL2
if [ ! -f /proc/version ] || ! grep -qi microsoft /proc/version; then
    echo -e "${YELLOW}Warning: This script is designed to run inside WSL2${NC}"
    echo "If you're on Windows, run this via: wsl bash scripts/build-wsl.sh"
fi

# Get the script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo -e "${GREEN}Project root: $PROJECT_ROOT${NC}"

# Check if Rust is installed
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}Error: Rust/Cargo not found${NC}"
    echo "Install Rust from: https://rustup.rs/"
    exit 1
fi

# Build exo in release mode
echo -e "${GREEN}Building exo...${NC}"
cargo build --release

# Check if build succeeded
if [ -f "target/release/exo" ]; then
    echo -e "${GREEN}✓ Build successful!${NC}"
    echo -e "${GREEN}Binary location: $PROJECT_ROOT/target/release/exo${NC}"

    # Show binary size
    SIZE=$(du -h target/release/exo | cut -f1)
    echo -e "${GREEN}Binary size: $SIZE${NC}"

    # Test run
    echo -e "${GREEN}Testing exo binary...${NC}"
    if target/release/exo --version &> /dev/null; then
        echo -e "${GREEN}✓ exo binary is working!${NC}"
    else
        echo -e "${YELLOW}Warning: exo binary may have issues${NC}"
    fi
else
    echo -e "${RED}✗ Build failed!${NC}"
    exit 1
fi

# Optional: Install to /usr/local/bin
echo ""
read -p "Install exo to /usr/local/bin? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${GREEN}Installing exo to /usr/local/bin...${NC}"
    sudo cp target/release/exo /usr/local/bin/exo
    sudo chmod +x /usr/local/bin/exo
    echo -e "${GREEN}✓ Installed! You can now run 'exo' from anywhere.${NC}"
fi

echo -e "${GREEN}=== Build Complete ===${NC}"
