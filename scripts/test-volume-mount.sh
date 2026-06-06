#!/bin/bash
# Test script for Exo volume mounting on WSL2
# Run this INSIDE WSL2 to test volume mounting

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Counters
PASS=0
FAIL=0

test_step() {
    local name="$1"
    local script="$2"

    echo -e "${CYAN}→ Testing: $name${NC}"
    if eval "$script"; then
        echo -e "${GREEN}✓ PASS: $name${NC}"
        ((PASS++))
    else
        echo -e "${RED}✗ FAIL: $name${NC}"
        ((FAIL++))
    fi
}

echo -e "${YELLOW}
╔════════════════════════════════════════════════════════════╗
║     Exo Volume Mount Test Suite for WSL2                  ║
╚════════════════════════════════════════════════════════════╝
${NC}"

# Detect project root
if [ -f "../target/release/exo" ]; then
    PROJECT_ROOT="$(cd .. && pwd)"
elif [ -f "target/release/exo" ]; then
    PROJECT_ROOT="$(pwd)"
else
    PROJECT_ROOT="/mnt/f/Software/exo"
fi

EXO_BIN="$PROJECT_ROOT/target/release/exo"
TEST_DIR="$PROJECT_ROOT/test-volumes"
TEST_DATA_DIR="$TEST_DIR/persistent-data"

echo -e "${CYAN}Project root: $PROJECT_ROOT${NC}"
echo -e "${CYAN}Exo binary: $EXO_BIN${NC}"

# ===== Environment Check =====
echo -e "\n${YELLOW}=== Environment Check ===${NC}"

test_step "Running in WSL2" "grep -qi microsoft /proc/version 2>/dev/null"

test_step "Exo binary exists" "[ -x '$EXO_BIN' ]"

# ===== Path Translation Tests =====
echo -e "\n${YELLOW}=== Path Translation Tests ===${NC}"

test_step "Convert Windows C: drive to WSL path" "
    echo 'C:\Users\test' | sed 's|\\|/|g' | sed 's|^\([A-Z]\):|/mnt/\L\1|' | grep -q '/mnt/c/Users/test'
"

test_step "Convert Windows F: drive to WSL path" "
    echo 'F:\Software\exo' | sed 's|\\|/|g' | sed 's|^\([A-Z]\):|/mnt/\L\1|' | grep -q '/mnt/f/Software'
"

# ===== Basic Volume Mount Tests =====
echo -e "\n${YELLOW}=== Basic Volume Mount Tests ===${NC}"

# Setup test directory
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"
mkdir -p "$TEST_DATA_DIR"

# Create test file
echo "Hello from Windows!" > "$TEST_DIR/test.txt"
echo -e "${GREEN}Created test directory: $TEST_DIR${NC}"

test_step "Read file in test directory" "
    grep -q 'Hello from Windows!' '$TEST_DIR/test.txt'
"

test_step "Write to test directory" "
    echo 'Written from WSL2' > '$TEST_DIR/wsl-wrote.txt' && grep -q 'Written from WSL2' '$TEST_DIR/wsl-wrote.txt'
"

test_step "Create nested directory structure" "
    mkdir -p '$TEST_DIR/nested/deep/path' && echo 'nested' > '$TEST_DIR/nested/deep/path/test.txt' && [ -f '$TEST_DIR/nested/deep/path/test.txt' ]
"

# ===== Exo Container Volume Tests =====
echo -e "\n${YELLOW}=== Exo Container Volume Tests ===${NC}"

if [ ! -x "$EXO_BIN" ]; then
    echo -e "${RED}Exo binary not found or not executable. Skipping container tests.${NC}"
    echo -e "${CYAN}Build with: cargo build --release${NC}"
else
    echo -e "${GREEN}Exo binary found${NC}"

    # Test: Basic volume mount
    test_step "Basic volume mount to container" "
        timeout 10 '$EXO_BIN' run --rm -v '$TEST_DIR:/data' alpine:latest cat /data/test.txt 2>&1 | grep -q 'Hello from Windows!'
    "

    # Test: Write to Windows volume from container
    test_step "Write to Windows volume from container" "
        rm -f '$TEST_DIR/container-wrote.txt'
        timeout 10 '$EXO_BIN' run --rm -v '$TEST_DIR:/data' alpine:latest sh -c 'echo Container data > /data/container-wrote.txt' 2>/dev/null
        sleep 0.5
        grep -q 'Container data' '$TEST_DIR/container-wrote.txt'
    "

    # Test: Persistent data across container restarts
    test_step "Persistent data survives container restart" "
        rm -f '$TEST_DATA_DIR/persistent.txt'
        # First run: write data
        timeout 10 '$EXO_BIN' run --rm -v '$TEST_DATA_DIR:/persist' alpine:latest sh -c 'echo First run > /persist/persistent.txt' 2>/dev/null
        sleep 0.5
        # Second run: read data
        timeout 10 '$EXO_BIN' run --rm -v '$TEST_DATA_DIR:/persist' alpine:latest cat /persist/persistent.txt 2>&1 | grep -q 'First run'
    "

    # Test: Multiple volume mounts
    test_step "Multiple volume mounts" "
        mkdir -p '$TEST_DIR/vol1' '$TEST_DIR/vol2'
        echo 'volume1' > '$TEST_DIR/vol1/data.txt'
        echo 'volume2' > '$TEST_DIR/vol2/data.txt'
        timeout 10 '$EXO_BIN' run --rm \
            -v '$TEST_DIR/vol1:/mnt/vol1' \
            -v '$TEST_DIR/vol2:/mnt/vol2' \
            alpine:latest sh -c 'cat /mnt/vol1/data.txt && cat /mnt/vol2/data.txt' 2>&1 | grep -q 'volume1.*volume2'
    "
fi

# ===== Edge Case Tests =====
echo -e "\n${YELLOW}=== Edge Case Tests ===${NC}"

test_step "Handle path with spaces" "
    mkdir -p '$TEST_DIR/path with spaces'
    echo 'Spaces work!' > '$TEST_DIR/path with spaces/test.txt'
    grep -q 'Spaces work!' '$TEST_DIR/path with spaces/test.txt'
"

test_step "Special characters in filename" "
    echo 'special' > '$TEST_DIR/file-with-special.chars.txt'
    grep -q 'special' '$TEST_DIR/file-with-special.chars.txt'
"

# ===== Performance Tests =====
echo -e "\n${YELLOW}=== Performance Tests ===${NC}"

test_step "Large file read (1MB)" "
    dd if=/dev/zero of='$TEST_DIR/large.txt' bs=1024 count=1024 2>/dev/null
    START=\$(date +%s%3N)
    SIZE=\$(wc -c < '$TEST_DIR/large.txt')
    END=\$(date +%s%3N)
    ELAPSED=\$((END - START))
    echo \"Read \$SIZE bytes in \${ELAPSED}ms\"
    [ \$SIZE -gt 1000000 ]
"

test_step "Many small files" "
    mkdir -p '$TEST_DIR/many'
    for i in \$(seq 1 100); do echo \"file \$i\" > '$TEST_DIR/many/file\$i.txt'; done
    START=\$(date +%s%3N)
    COUNT=\$(ls '$TEST_DIR/many' 2>/dev/null | wc -l)
    END=\$(date +%s%3N)
    ELAPSED=\$((END - START))
    echo \"Listed \$COUNT files in \${ELAPSED}ms\"
    [ \$COUNT -ge 100 ]
"

# ===== Cleanup =====
echo -e "\n${YELLOW}=== Cleanup ===${NC}"

rm -rf "$TEST_DIR"
echo -e "${GREEN}✓ Test directory cleaned up${NC}"

# ===== Summary =====
echo -e "\n${YELLOW}=== Test Results Summary ===${NC}"
echo ""
echo -e "Total Tests: $((PASS + FAIL))"
echo -e "Passed:      ${GREEN}$PASS${NC}"
echo -e "Failed:      ${RED}$FAIL${NC}"
echo ""

if [ $FAIL -eq 0 ]; then
    echo -e "${GREEN}🎉 All tests passed!${NC}"
    echo ""
    echo -e "${CYAN}Volume mounting is working correctly on your WSL2 setup.${NC}"
    exit 0
else
    echo -e "${YELLOW}⚠️  Some tests failed. Check the output above for details.${NC}"
    echo ""
    echo -e "${YELLOW}Common issues:${NC}"
    echo "  - Exo binary may need to be rebuilt: cargo build --release"
    echo "  - Check if exo has permission to create namespaces"
    echo "  - Some tests require Alpine image: docker pull alpine"
    exit 1
fi
