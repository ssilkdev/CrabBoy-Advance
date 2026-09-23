#!/usr/bin/env bash
# Download the freely redistributable hardware test ROMs used by
# tests/gba_test_roms.rs and tests/gb_test_roms.rs (ROADMAP M1).
#
#   scripts/fetch_test_roms.sh [DEST]      (default: ./test-roms)
#
# Then run the harnesses against them:
#
#   GBA_TEST_ROM_DIR=DEST/gba GB_TEST_ROM_DIR=DEST/gb \
#   CRABBOY_REQUIRE_TEST_ROMS=1 cargo test --release --test gba_test_roms --test gb_test_roms
#
# CRABBOY_REQUIRE_TEST_ROMS=1 turns "ROM missing, skipping" into a failure,
# which is what CI wants: a broken download must not look like a pass.
#
# Sources (all free to download; none are vendored in this repo):
#   - jsmolka/gba-tests (MIT)                 GBA CPU/memory/save/PPU tests
#   - mGBA test suite (mgba-emu/suite)        GBA timing/timers/DMA/IO tests
#   - Blargg's GB tests (retrio/gb-test-roms) cpu_instrs, instr/mem timing
#   - Matt Currie's dmg-acid2 / cgb-acid2 (MIT) PPU rendering
# The AGS aging cartridge is Nintendo-proprietary and cannot be fetched;
# the harness picks it up from $GBA_TEST_ROM_DIR/ags.gba if you supply it.
set -euo pipefail

DEST="${1:-test-roms}"
mkdir -p "$DEST/gba" "$DEST/gb"

fetch() { # url out
    if [[ -s "$2" ]]; then return; fi
    echo "fetch $1"
    curl -fsSL --retry 3 -o "$2.tmp" "$1"
    mv "$2.tmp" "$2"
}

# --- GBA -------------------------------------------------------------------
if [[ ! -d "$DEST/gba/gba-tests" ]]; then
    git clone -q --depth 1 https://github.com/jsmolka/gba-tests.git "$DEST/gba/gba-tests"
fi

if [[ ! -s "$DEST/gba/suite.gba" ]]; then
    fetch https://s3.amazonaws.com/mgba/suite-latest.zip "$DEST/gba/suite.zip"
    python3 - "$DEST/gba" <<'EOF'
import sys, zipfile, pathlib
dest = pathlib.Path(sys.argv[1])
with zipfile.ZipFile(dest / "suite.zip") as z:
    name = next(n for n in z.namelist() if n.endswith(".gba"))
    (dest / "suite.gba").write_bytes(z.read(name))
EOF
    rm -f "$DEST/gba/suite.zip"
fi

# --- GB --------------------------------------------------------------------
BLARGG=https://raw.githubusercontent.com/retrio/gb-test-roms/master
fetch "$BLARGG/cpu_instrs/cpu_instrs.gb" "$DEST/gb/cpu_instrs.gb"
fetch "$BLARGG/instr_timing/instr_timing.gb" "$DEST/gb/instr_timing.gb"
fetch "$BLARGG/mem_timing/mem_timing.gb" "$DEST/gb/mem_timing.gb"
fetch https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb "$DEST/gb/dmg-acid2.gb"
fetch https://github.com/mattcurrie/cgb-acid2/releases/download/v1.1/cgb-acid2.gbc "$DEST/gb/cgb-acid2.gbc"

echo "Test ROMs ready in $DEST"
ls -la "$DEST/gba" "$DEST/gb"
